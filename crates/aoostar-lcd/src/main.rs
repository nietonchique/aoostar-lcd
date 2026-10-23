//! Granite front-panel status display.

mod collect;
mod config;
mod model;
mod render;
mod shutdown;

use anyhow::Context;
use asterctl_lcd::AooScreenBuilder;
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::collect::{Collector, collect_once};
use crate::config::Config;
use crate::model::Snapshot;
use crate::render::{Rotation, render, render_all};

const DEFAULT_CONFIG: &str = "/etc/aoostar-lcd.toml";
const FRAME_INTERVAL: Duration = Duration::from_secs(1);
const FULL_REFRESH: Duration = Duration::from_secs(30);

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Drive the LCD: collect, render and send one frame per second.
    Run {
        #[arg(long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
    },
    /// Validate the configuration and run one full collection (never touches the LCD).
    Check {
        #[arg(long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
    },
    /// Print one complete snapshot as JSON (input for `preview`).
    Snapshot {
        #[arg(long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
    },
    /// Render every page of a snapshot JSON to `<out-dir>/<page>.png`.
    Preview {
        #[arg(long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long)]
        fixture: PathBuf,
        #[arg(long)]
        out_dir: PathBuf,
    },
    /// Switch the LCD off.
    Off {
        #[arg(long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
    },
    /// Show shutdown/reboot progress until the watched services have stopped.
    /// With no shutdown in progress, switches the panel off immediately.
    Shutdown {
        #[arg(long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        /// Give up after this many seconds even if services are still stopping.
        #[arg(long, default_value_t = 80)]
        max_secs: u64,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Run { config } => run(&Config::load(&config)?),
        Command::Check { config } => check(&Config::load(&config)?),
        Command::Snapshot { config } => {
            let snapshot = collect_once(&Config::load(&config)?);
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
            Ok(())
        }
        Command::Preview {
            config,
            fixture,
            out_dir,
        } => preview(&Config::load(&config)?, &fixture, &out_dir),
        Command::Off { config } => {
            let config = Config::load(&config)?;
            let mut screen = AooScreenBuilder::new().open_device(&device(&config))?;
            screen.off()
        }
        Command::Shutdown { config, max_secs } => shutdown::run(&Config::load(&config)?, max_secs),
    }
}

fn device(config: &Config) -> String {
    config.display.device.to_string_lossy().into_owned()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn run(config: &Config) -> anyhow::Result<()> {
    let mut screen = AooScreenBuilder::new()
        .open_device(&device(config))
        .context("opening LCD")?;
    screen.init().context("initialising LCD")?;
    let strings = render::i18n::I18n::load(config)?;
    let mut collector = Collector::start(config);
    let mut rotation = Rotation::new(Instant::now());
    let mut heartbeat_failed = false;
    let mut flash = false;
    let mut last_page = None;
    let mut last_full = Instant::now();
    eprintln!("aoostar-lcd {} running", env!("CARGO_PKG_VERSION"));
    loop {
        let tick = Instant::now();
        let snapshot = collector.snapshot();
        let (page, index, count) = rotation.tick(&snapshot, config, &strings, tick);
        // The panel occasionally drops chunks of a differential update, which
        // leaves fragments of the previous page behind. Send a full frame on
        // every page change and at least every FULL_REFRESH to heal that.
        if last_page != Some(page) || tick.duration_since(last_full) >= FULL_REFRESH {
            screen.clear_cache();
            last_full = tick;
            last_page = Some(page);
        }
        // A full-screen alarm repaint takes ~1.3 s on this panel; flipping the
        // phase per frame keeps the blink regular regardless of frame time.
        flash = !flash;
        let frame = render(
            page,
            &snapshot,
            config,
            unix_now(),
            (index, count),
            flash,
            &strings,
        );
        // A write error usually means the USB device went away; exit and let
        // systemd restart the service once the device is back.
        screen.send_image(&frame).context("sending frame")?;
        if let Err(error) = std::fs::write(&config.display.heartbeat, unix_now().to_string()) {
            if !heartbeat_failed {
                eprintln!(
                    "cannot write heartbeat {}: {error}",
                    config.display.heartbeat.display()
                );
            }
            heartbeat_failed = true;
        } else {
            heartbeat_failed = false;
        }
        std::thread::sleep(FRAME_INTERVAL.saturating_sub(tick.elapsed()));
    }
}

fn check(config: &Config) -> anyhow::Result<()> {
    render::canvas::check_fonts().context("embedded fonts")?;
    let snapshot = collect_once(config);
    for (page, frame) in render_all(&snapshot, config, snapshot.taken_at) {
        anyhow::ensure!(
            frame.dimensions() == asterctl_lcd::DISPLAY_SIZE,
            "page {} rendered at {:?}",
            page.slug(),
            frame.dimensions()
        );
    }
    let present = snapshot.disks.iter().filter(|disk| disk.present).count();
    println!(
        "config ok; disks {present}/{}; volumes {}; problems {}; alerts {:?}; ci running {}",
        snapshot.disks.len(),
        snapshot.volumes.len(),
        snapshot.health.problems.len(),
        snapshot.health.alerts,
        snapshot.ci.running.len(),
    );
    Ok(())
}

fn preview(config: &Config, fixture: &Path, out_dir: &Path) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(fixture)
        .with_context(|| format!("reading {}", fixture.display()))?;
    let snapshot: Snapshot = serde_json::from_str(&text).context("parsing snapshot")?;
    std::fs::create_dir_all(out_dir)?;
    let now = if snapshot.taken_at > 0 {
        snapshot.taken_at
    } else {
        unix_now()
    };
    for (page, frame) in render_all(&snapshot, config, now) {
        let path = out_dir.join(format!("{}.png", page.slug()));
        frame
            .save(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        println!("{}", path.display());
    }
    Ok(())
}
