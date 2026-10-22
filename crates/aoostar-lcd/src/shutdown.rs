//! Shutdown/reboot progress display: `aoostar-lcd shutdown`.
//!
//! Run from `ExecStopPost` while systemd tears the system down. It shows the
//! watched services stopping — how many have stopped, which are still up,
//! how many containers remain — once per second, for up to `max_secs`. With
//! no shutdown job pending (a manual unit stop or a deploy restart) it just
//! switches the panel off, like the previous `aoostar-lcd off` hook. A
//! poweroff ends with the panel dark; after a reboot the last frame stays on
//! until the startup page replaces it after boot.

use std::time::{Duration, Instant};

use anyhow::Context;
use asterctl_lcd::AooScreenBuilder;

use crate::collect::{docker, systemd};
use crate::config::Config;

/// One frame per second; a failing source keeps its last known value.
const POLL: Duration = Duration::from_secs(1);

/// How many watched services have stopped and which have not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub total: usize,
    pub stopped: usize,
    /// Watched services systemd still reports active.
    pub pending: Vec<String>,
    /// Watched services in the `failed` state.
    pub failed: Vec<String>,
}

/// Progress of the watched services from the latest systemd state.
pub fn progress(state: &systemd::SystemdState) -> Progress {
    let mut watched: Vec<&systemd::UnitState> =
        state.stacks.iter().chain(state.units.iter()).collect();
    watched.sort_by(|a, b| a.subject.cmp(&b.subject));
    let pending: Vec<String> = watched
        .iter()
        .filter(|unit| unit.is_active())
        .map(|unit| unit.subject.clone())
        .collect();
    let failed = watched
        .iter()
        .filter(|unit| unit.active == "failed")
        .map(|unit| unit.subject.clone())
        .collect();
    Progress {
        total: watched.len(),
        stopped: watched.len() - pending.len(),
        pending,
        failed,
    }
}

fn screen(config: &Config) -> anyhow::Result<asterctl_lcd::AooScreen> {
    let mut screen = AooScreenBuilder::new()
        .open_device(&crate::device(config))
        .context("opening LCD")?;
    screen.init().context("initialising LCD")?;
    Ok(screen)
}

fn switch_off(config: &Config) -> anyhow::Result<()> {
    AooScreenBuilder::new()
        .open_device(&crate::device(config))
        .context("opening LCD")?
        .off()
        .context("switching LCD off")
}

pub fn run(config: &Config, max_secs: u64) -> anyhow::Result<()> {
    let Some(target) = systemd::shutdown_target() else {
        // Nothing is shutting down: a manual stop or a deploy restart.
        return switch_off(config);
    };
    let strings = crate::render::i18n::Strings::get(config.display.language);
    let mut screen = screen(config)?;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(max_secs);
    let mut last = Progress::default();
    let mut have_state = false;
    let mut containers: Option<u32> = None;
    loop {
        if let Ok(state) = systemd::read(&config.services) {
            last = progress(&state);
            have_state = true;
        }
        if let Ok(running) = docker::running_count(&config.services.docker_socket) {
            containers = Some(running);
        }
        let done = have_state && last.pending.is_empty();
        let frame = crate::render::shutdown_page(
            target,
            strings,
            &last,
            have_state,
            containers,
            started.elapsed().as_secs(),
            done,
        );
        screen.send_image(&frame).context("sending frame")?;
        if done || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(POLL);
    }
    // The panel keeps the last frame across the reboot gap on purpose: it
    // names the transition until the startup page replaces it after boot.
    // After a poweroff most boards cut USB power; switch the panel off in
    // case this one does not.
    if target == systemd::Target::Poweroff {
        screen.off().context("switching LCD off")?;
    }
    Ok(())
}
