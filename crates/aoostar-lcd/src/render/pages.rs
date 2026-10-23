//! Page layouts. Every page shares the status bar; bodies are tuned for
//! legibility from a distance: few items, big numbers, colour only for state.

use image::RgbImage;

use crate::collect::systemd::Target;
use crate::config::{Config, Level, Tone};
use crate::model::{DiskKind, Problem, ProblemClass, ScanKind, Severity, Snapshot, SourceState};
use crate::shutdown::Progress;

use super::Page;
use super::canvas::{Align, Area, Canvas, Style, Weight, fit};
use super::i18n::I18n;
use super::theme::*;

const WIDTH: f32 = 960.0;
const HEIGHT: f32 = 376.0;
const MARGIN: f32 = 8.0;
const GAP: f32 = 8.0;
const BAR_HEIGHT: f32 = 64.0;
const BODY_TOP: f32 = BAR_HEIGHT + GAP;
const BODY_HEIGHT: f32 = HEIGHT - BODY_TOP - MARGIN;
const PAD: f32 = 18.0;
const RADIUS: f32 = 14.0;

pub fn render(
    page: Page,
    snapshot: &Snapshot,
    config: &Config,
    now_unix: i64,
    position: (usize, usize),
    flash: bool,
    strings: &I18n,
) -> RgbImage {
    let view = View {
        snapshot,
        config,
        now: now_unix,
        flash,
        strings,
    };
    if page == Page::Alarm {
        return alarm(&view).into_rgb();
    }
    if page == Page::Startup {
        return startup(&view).into_rgb();
    }
    let mut canvas = Canvas::new(WIDTH as u32, HEIGHT as u32, BACKGROUND);
    status_bar(&mut canvas, &view, position);
    match page {
        Page::Alarm | Page::Startup => unreachable!("handled above"),
        Page::Home => home(&mut canvas, &view),
        Page::Problems => problems(&mut canvas, &view),
        Page::Ci => ci(&mut canvas, &view),
        Page::Hdd => hdd(&mut canvas, &view),
        Page::Zfs => zfs(&mut canvas, &view),
        Page::Temps => temps(&mut canvas, &view),
        Page::Load => load(&mut canvas, &view),
        Page::Space => space(&mut canvas, &view),
    }
    canvas.into_rgb()
}

struct View<'a> {
    snapshot: &'a Snapshot,
    config: &'a Config,
    now: i64,
    /// Alarm blink phase; the caller flips it every frame.
    flash: bool,
    /// Localized UI strings for the configured language.
    strings: &'a I18n,
}

impl View<'_> {
    fn tone(&self, value: Option<f32>, level: Level) -> Tone {
        value.map_or(Tone::Normal, |value| level.classify(value))
    }

    fn memory_used_percent(&self) -> Option<f32> {
        let host = &self.snapshot.host;
        (host.mem_total > 0).then(|| {
            host.mem_total.saturating_sub(host.mem_available) as f32 * 100.0 / host.mem_total as f32
        })
    }

    fn longest_pipeline_secs(&self) -> Option<i64> {
        self.snapshot
            .ci
            .running
            .iter()
            .map(|pipeline| self.now - pipeline.started_at)
            .max()
    }

    fn ci_tone(&self, seconds: i64) -> Tone {
        self.config
            .thresholds
            .ci_minutes
            .classify(seconds as f32 / 60.0)
    }

    fn hdd_temp_max(&self) -> Option<f32> {
        self.snapshot
            .disks
            .iter()
            .filter(|disk| disk.kind == DiskKind::Hdd)
            .filter_map(|disk| disk.temp_c)
            .reduce(f32::max)
    }
}

// ---------------------------------------------------------------- status bar

struct Chip {
    text: String,
    color: Rgb,
}

fn status_bar(canvas: &mut Canvas, view: &View, (index, count): (usize, usize)) {
    let strings = view.strings;
    let health = &view.snapshot.health;
    let active: Vec<&Problem> = health.active_outages().collect();
    let maintenance: Vec<&Problem> = health
        .outages()
        .filter(|problem| problem.severity == Severity::Maintenance)
        .collect();

    let (background, dot, text_color, text) = if !active.is_empty() {
        let critical = active
            .iter()
            .any(|problem| problem.severity == Severity::Critical);
        let names = subjects(strings, &active);
        if critical {
            (
                BAR_CRIT,
                TEXT,
                TEXT,
                format!("{}: {names}", strings.t("status.down")),
            )
        } else {
            (
                BAR_WARN,
                AMBER,
                AMBER,
                format!("{}: {names}", strings.t("status.problem")),
            )
        }
    } else if !maintenance.is_empty() {
        (
            BAR_MAINT,
            GREY,
            TEXT,
            format!(
                "{}: {}",
                strings.t("status.maintenance"),
                subjects(strings, &maintenance)
            ),
        )
    } else {
        (
            BAR_OK,
            GREEN,
            GREEN,
            strings.t("status.all_running").to_owned(),
        )
    };
    canvas.rect(0.0, 0.0, WIDTH, BAR_HEIGHT, background);

    // Right side, from the edge inwards: clock, page dots, chips.
    let clock_style = TITLE.color(if background == BAR_CRIT { TEXT } else { MUTED });
    let clock = clock_text(view.now, view.config.display.clock_utc_offset_minutes);
    let baseline = BAR_HEIGHT / 2.0 + Canvas::cap_height(clock_style) / 2.0;
    let clock_width = canvas.text(WIDTH - PAD, baseline, Align::Right, clock_style, &clock);
    let mut right = WIDTH - PAD - clock_width - 22.0;

    if count > 1 {
        let step = 20.0;
        let first = right - step * (count as f32 - 1.0) - 6.0;
        for dot_index in 0..count {
            let x = first + step * dot_index as f32;
            let (radius, color) = if dot_index == index {
                (7.0, TEXT)
            } else {
                (5.0, DIM)
            };
            canvas.circle(x, BAR_HEIGHT / 2.0, radius, color);
        }
        right = first - 26.0;
    }

    // During a critical outage the names of what is down need all the room.
    let chips = if background != BAR_CRIT {
        chips(view)
    } else {
        Vec::new()
    };
    for chip in chips.iter().rev() {
        let style = DETAIL.color(chip.color).size(38.0);
        let width = Canvas::measure(style, &chip.text) + 28.0;
        let left = right - width;
        if left < 360.0 {
            break;
        }
        canvas.rounded_rect(left, 12.0, width, BAR_HEIGHT - 24.0, 10.0, BACKGROUND);
        let chip_baseline = BAR_HEIGHT / 2.0 + Canvas::cap_height(style) / 2.0;
        canvas.text(left + 14.0, chip_baseline, Align::Left, style, &chip.text);
        right = left - 10.0;
    }

    canvas.circle(PAD + 12.0, BAR_HEIGHT / 2.0, 12.0, dot);
    let style = TITLE.color(text_color);
    let left = PAD + 36.0;
    let (fitted, label) = fit(style, MIN_SIZE, right - left - 8.0, &text);
    let text_baseline = BAR_HEIGHT / 2.0 + Canvas::cap_height(fitted) / 2.0;
    canvas.text(left, text_baseline, Align::Left, fitted, &label);
}

fn subjects(strings: &I18n, problems: &[&Problem]) -> String {
    let mut names: Vec<String> = Vec::new();
    for problem in problems {
        let subject = strings.subject(&problem.subject);
        if !names.iter().any(|known| known.as_str() == subject) {
            names.push(subject.to_owned());
        }
    }
    names.join(", ")
}

fn chips(view: &View) -> Vec<Chip> {
    let strings = view.strings;
    let mut chips = Vec::new();
    let snapshot = view.snapshot;
    let alerts = snapshot.health.alerts().count();
    if alerts > 0 {
        chips.push(Chip {
            text: format!("{} {alerts}", strings.t("status.alerts")),
            color: AMBER,
        });
    }
    if let SourceState::Unavailable(_) = snapshot.health.alerts {
        chips.push(Chip {
            text: strings.t("status.alerts_unknown").to_owned(),
            color: AMBER,
        });
    }
    if let Some(longest) = view.longest_pipeline_secs() {
        chips.push(Chip {
            text: format!(
                "CI {} · {}",
                snapshot.ci.running.len(),
                strings.stopwatch_short(longest)
            ),
            color: match view.ci_tone(longest) {
                Tone::Normal => BLUE,
                tone => tone_color(tone),
            },
        });
    }
    if super::hdd_relevant(snapshot, view.config) {
        chips.push(Chip {
            text: format!("HDD {:.0}%", snapshot.hdd_busy_max()),
            color: tone_color(
                view.config
                    .thresholds
                    .hdd_busy_percent
                    .classify(snapshot.hdd_busy_max())
                    .max_warning(),
            ),
        });
    }
    chips
}

trait ToneExt {
    fn max_warning(self) -> Tone;
}

impl ToneExt for Tone {
    /// Activity is never "critical" in the status bar; it is a hint.
    fn max_warning(self) -> Tone {
        match self {
            Tone::Critical => Tone::Warning,
            tone => tone,
        }
    }
}

fn clock_text(now: i64, offset_minutes: i32) -> String {
    let local = now + i64::from(offset_minutes) * 60;
    let minutes_of_day = local.rem_euclid(86_400) / 60;
    format!("{:02}:{:02}", minutes_of_day / 60, minutes_of_day % 60)
}

// ---------------------------------------------------------------- building blocks

struct Tile {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Tile {
    fn grid(columns: usize, rows: usize, top: f32, height: f32) -> Vec<Tile> {
        let w = (WIDTH - 2.0 * MARGIN - GAP * (columns as f32 - 1.0)) / columns as f32;
        let h = (height - GAP * (rows as f32 - 1.0)) / rows as f32;
        (0..rows)
            .flat_map(|row| {
                (0..columns).map(move |column| Tile {
                    x: MARGIN + column as f32 * (w + GAP),
                    y: top + row as f32 * (h + GAP),
                    w,
                    h,
                })
            })
            .collect()
    }

    fn panel(&self, canvas: &mut Canvas) {
        canvas.rounded_rect(self.x, self.y, self.w, self.h, RADIUS, PANEL);
    }

    fn inner(&self) -> f32 {
        self.w - 2.0 * PAD
    }
}

/// Big number with a smaller unit sharing its baseline, left aligned.
fn value_with_unit(
    canvas: &mut Canvas,
    x: f32,
    baseline: f32,
    max_width: f32,
    style: Style,
    value: &str,
    unit: &str,
) {
    let unit_style = LABEL.color(style.color);
    let unit_width = if unit.is_empty() {
        0.0
    } else {
        Canvas::measure(unit_style, unit) + 8.0
    };
    let (fitted, text) = fit(style, 40.0, max_width - unit_width, value);
    let width = canvas.text(x, baseline, Align::Left, fitted, &text);
    if !unit.is_empty() {
        canvas.text(x + width + 8.0, baseline, Align::Left, unit_style, unit);
    }
}

/// Wide headline tile: big value on the left (with a meter under it), label
/// and two detail lines in a right-hand column so the width carries text.
struct Headline<'a> {
    label: String,
    value: String,
    unit: &'a str,
    tone: Tone,
    lines: [String; 2],
    meter: Option<(f32, Tone)>,
}

const HEADLINE_VALUE_WIDTH: f32 = 214.0;

fn headline(canvas: &mut Canvas, tile: &Tile, item: Headline) {
    tile.panel(canvas);
    let x = tile.x + PAD;
    let value_baseline = tile.y + (tile.h - 14.0 + Canvas::cap_height(VALUE_XL)) / 2.0;
    value_with_unit(
        canvas,
        x - 3.0,
        value_baseline,
        HEADLINE_VALUE_WIDTH + 3.0,
        VALUE_XL.color(tone_color(item.tone)),
        &item.value,
        item.unit,
    );
    if let Some((fraction, tone)) = item.meter {
        canvas.meter(
            Area::new(x, tile.y + tile.h - 16.0, HEADLINE_VALUE_WIDTH, 8.0),
            fraction,
            tone_fill(tone),
            TRACK,
        );
    }

    let column = x + HEADLINE_VALUE_WIDTH + 16.0;
    let width = tile.x + tile.w - PAD - column;
    let label_baseline = tile.y + 14.0 + Canvas::cap_height(LABEL);
    canvas.text_fit(
        column,
        label_baseline,
        Align::Left,
        LABEL,
        width,
        &item.label,
    );
    // The column has room for larger detail text than the panel minimum.
    let line_style = DETAIL.color(TEXT).size(42.0);
    let mut baseline = label_baseline;
    for line in &item.lines {
        baseline += Canvas::cap_height(line_style) + 14.0;
        if !line.is_empty() {
            canvas.text_fit(column, baseline, Align::Left, line_style, width, line);
        }
    }
}

fn empty_message(canvas: &mut Canvas, text: &str) {
    let style = TITLE.color(MUTED);
    canvas.text(
        WIDTH / 2.0,
        BODY_TOP + BODY_HEIGHT / 2.0 + Canvas::cap_height(style) / 2.0,
        Align::Center,
        style,
        text,
    );
}

/// A panel row card; `accent` draws the colored stripe on the left edge.
fn card(canvas: &mut Canvas, x: f32, y: f32, w: f32, h: f32, accent: Option<Rgb>) {
    canvas.rounded_rect(x, y, w, h, RADIUS, PANEL);
    if let Some(color) = accent {
        canvas.rounded_rect(x, y, 12.0, h, 6.0, color);
    }
}

/// Vertical baseline of a single line centered inside a row of height `h`.
/// The baseline is the bottom of the caps: centering it means half the cap
/// height ends up below the middle line.
fn center_baseline(y: f32, h: f32, style: Style) -> f32 {
    y + (h + Canvas::cap_height(style)) / 2.0
}

/// Baselines of a vertically centered title-over-detail block. Row content
/// must not hug the bottom edge: the block shares the top and bottom padding.
fn two_line_baselines(y: f32, h: f32, title: Style, detail: Style, gap: f32) -> (f32, f32) {
    let block = Canvas::cap_height(title) + gap + Canvas::cap_height(detail);
    let top = y + (h - block) / 2.0;
    (
        top + Canvas::cap_height(title),
        top + Canvas::cap_height(title) + gap + Canvas::cap_height(detail),
    )
}

// ---------------------------------------------------------------- pages

fn home(canvas: &mut Canvas, view: &View) {
    let strings = view.strings;
    let snapshot = view.snapshot;
    let host = &snapshot.host;
    let thresholds = &view.config.thresholds;
    let tiles = Tile::grid(2, 2, BODY_TOP, BODY_HEIGHT);

    let cpu_load = host.cpu_percent;
    headline(
        canvas,
        &tiles[0],
        Headline {
            label: "CPU".to_owned(),
            value: temperature(host.cpu_temp_c),
            unit: "",
            tone: view.tone(host.cpu_temp_c, thresholds.cpu_temp_c),
            lines: [
                format!("{} {}", strings.t("home.load"), percent(cpu_load)),
                host.load1
                    .map_or_else(String::new, |load| format!("LA {load:.1}")),
            ],
            meter: cpu_load.map(|load| (load / 100.0, Level(85.0, 95.0).classify(load))),
        },
    );

    let used = view.memory_used_percent();
    let (free_value, free_unit) = strings.split_unit(&strings.bytes(host.mem_available));
    let swap = if host.swap_used > 64 * 1024 * 1024 {
        format!("swap {}", strings.bytes(host.swap_used))
    } else {
        String::new()
    };
    headline(
        canvas,
        &tiles[1],
        Headline {
            label: strings.t("home.free_ram").to_owned(),
            value: if host.mem_total > 0 {
                free_value
            } else {
                "--".to_owned()
            },
            unit: if host.mem_total > 0 { free_unit } else { "" },
            tone: view.tone(used, thresholds.memory_used_percent),
            lines: [
                format!("{} {}", strings.t("home.of"), strings.bytes(host.mem_total)),
                swap,
            ],
            meter: used.map(|used| (used / 100.0, thresholds.memory_used_percent.classify(used))),
        },
    );

    let gpu = host.gpu.as_ref();
    let gpu_temp = gpu.and_then(|gpu| gpu.temp_c);
    let gpu_util = gpu.and_then(|gpu| gpu.util_percent);
    let vram = gpu.and_then(|gpu| match (gpu.mem_used_mib, gpu.mem_total_mib) {
        (Some(used), Some(total)) if total > 0 => Some(used as f32 * 100.0 / total as f32),
        _ => None,
    });
    headline(
        canvas,
        &tiles[2],
        Headline {
            label: "GPU".to_owned(),
            value: temperature(gpu_temp),
            unit: "",
            tone: view.tone(gpu_temp, thresholds.gpu_temp_c),
            lines: [
                match (gpu_util, gpu.and_then(|gpu| gpu.power_w)) {
                    (Some(util), Some(power)) => {
                        format!("{util:.0}% · {power:.0} {}", strings.t("units.watt"))
                    }
                    (Some(util), None) => {
                        format!("{} {util:.0}%", strings.t("home.load"))
                    }
                    _ => String::new(),
                },
                vram.map_or_else(String::new, |vram| format!("VRAM {vram:.0}%")),
            ],
            meter: gpu_util.map(|util| (util / 100.0, Tone::Normal)),
        },
    );

    let busy = snapshot.hdd_busy_max();
    let has_hdd = snapshot.disks.iter().any(|disk| disk.kind == DiskKind::Hdd);
    let busy_tone = thresholds.hdd_busy_percent.classify(busy).max_warning();
    let hdd_busy = busy >= thresholds.hdd_busy_percent.0;
    let (read, write) = snapshot
        .disks
        .iter()
        .filter(|disk| disk.kind == DiskKind::Hdd)
        .fold((0_u64, 0_u64), |(read, write), disk| {
            (read + disk.read_bps, write + disk.write_bps)
        });
    // Busy: say plainly how much is read and how much is written.
    let lines = if hdd_busy {
        [
            format!("{} {}", strings.t("home.read"), megabytes(read)),
            format!("{} {}", strings.t("home.write"), megabytes(write)),
        ]
    } else {
        let second = match snapshot.pools.iter().find_map(|pool| pool.scan.as_ref()) {
            Some(scan) if scan.paused => {
                format!("{} {}", scan_name(scan.kind), strings.t("home.paused"))
            }
            Some(scan) => format!("{} {:.0}%", scan_name(scan.kind), scan.percent),
            None if snapshot.hdd_self_test_running() => strings.t("home.smart_test").to_owned(),
            None => view.hdd_temp_max().map_or_else(String::new, |temp| {
                format!("{} {}°", strings.t("home.max"), temp)
            }),
        };
        [strings.t("home.idle").to_owned(), second]
    };
    headline(
        canvas,
        &tiles[3],
        Headline {
            label: if hdd_busy {
                format!("HDD, {}", strings.t("units.rate"))
            } else {
                "HDD".to_owned()
            },
            value: if has_hdd {
                format!("{busy:.0}%")
            } else {
                "--".to_owned()
            },
            unit: "",
            tone: busy_tone,
            lines,
            meter: has_hdd.then_some((busy / 100.0, busy_tone)),
        },
    );
}

/// `read 76`, `write 120` or `read 5, write 3` (MB/s, unit added by the caller).
fn read_write(strings: &I18n, read_bps: u64, write_bps: u64) -> String {
    let read = megabytes(read_bps);
    let write = megabytes(write_bps);
    let read_line = format!("{} {read}", strings.t("home.read"));
    let write_line = format!("{} {write}", strings.t("home.write"));
    match (read.as_str(), write.as_str()) {
        (_, "0") => read_line,
        ("0", _) => write_line,
        _ => format!("{read_line}, {write_line}"),
    }
}

fn scan_name(kind: ScanKind) -> &'static str {
    match kind {
        ScanKind::Scrub => "scrub",
        ScanKind::Resilver => "resilver",
    }
}

fn severity_color(problem: &Problem) -> Rgb {
    match (problem.class, problem.severity) {
        (_, Severity::Maintenance) => GREY,
        (ProblemClass::Outage, Severity::Critical) => RED,
        _ => AMBER,
    }
}

fn problems(canvas: &mut Canvas, view: &View) {
    let strings = view.strings;
    let list = &view.snapshot.health.problems;
    if list.is_empty() {
        empty_message(canvas, strings.t("problems.none"));
        return;
    }
    const ROWS: usize = 3;
    let row_height = (BODY_HEIGHT - GAP * (ROWS as f32 - 1.0)) / ROWS as f32;
    let overflow = list.len() > ROWS;
    let shown = if overflow { ROWS - 1 } else { list.len() };

    for (row, problem) in list.iter().take(shown).enumerate() {
        let y = BODY_TOP + row as f32 * (row_height + GAP);
        card(
            canvas,
            MARGIN,
            y,
            WIDTH - 2.0 * MARGIN,
            row_height,
            Some(severity_color(problem)),
        );

        let right_edge = WIDTH - MARGIN - PAD;
        let age_style = LABEL.color(TEXT);
        let (title_baseline, detail_baseline) =
            two_line_baselines(y, row_height, TITLE, DETAIL, 12.0);
        let age_width = match problem.since {
            Some(since) => canvas.text(
                right_edge,
                title_baseline,
                Align::Right,
                age_style,
                &strings.age(view.now - since),
            ),
            None => 0.0,
        };
        let left = MARGIN + 34.0;
        let title_style = TITLE.color(match problem.class {
            ProblemClass::Outage if problem.severity == Severity::Critical => RED,
            _ => TEXT,
        });
        canvas.text_fit(
            left,
            title_baseline,
            Align::Left,
            title_style,
            right_edge - age_width - 24.0 - left,
            strings.subject(&problem.subject),
        );
        let detail = strings.detail(&problem.detail);
        let count = if problem.count > 1 {
            canvas.text(
                right_edge,
                detail_baseline,
                Align::Right,
                DETAIL,
                &format!("×{}", problem.count),
            ) + 20.0
        } else {
            0.0
        };
        canvas.text_fit(
            left,
            detail_baseline,
            Align::Left,
            DETAIL,
            right_edge - count - left,
            &detail,
        );
    }

    if overflow {
        let rest = &list[shown..];
        let y = BODY_TOP + shown as f32 * (row_height + GAP);
        let worst = rest
            .iter()
            .map(severity_color)
            .find(|color| *color == RED)
            .unwrap_or(AMBER);
        card(
            canvas,
            MARGIN,
            y,
            WIDTH - 2.0 * MARGIN,
            row_height,
            Some(worst),
        );
        let left = MARGIN + 34.0;
        let (title_baseline, detail_baseline) =
            two_line_baselines(y, row_height, TITLE, DETAIL, 12.0);
        let total: u32 = rest.iter().map(|problem| problem.count.max(1)).sum();
        canvas.text(
            left,
            title_baseline,
            Align::Left,
            TITLE,
            &strings.tf(
                "problems.more_line",
                &[
                    ("count", &rest.len().to_string()),
                    ("alerts", &total.to_string()),
                ],
            ),
        );
        let names: Vec<String> = rest
            .iter()
            .map(|problem| strings.subject(&problem.subject).to_owned())
            .collect();
        canvas.text_fit(
            left,
            detail_baseline,
            Align::Left,
            DETAIL,
            WIDTH - MARGIN - PAD - left,
            &names.join(", "),
        );
    }
}

fn ci(canvas: &mut Canvas, view: &View) {
    let strings = view.strings;
    let ci = &view.snapshot.ci;
    const ROWS: usize = 3;
    let row_height = (BODY_HEIGHT - GAP * (ROWS as f32 - 1.0)) / ROWS as f32;
    let queued = ci.queued.unwrap_or(0);
    // The queue gets its own row when something waits; pipelines fill the rest.
    let pipeline_rows = if queued > 0 { ROWS - 1 } else { ROWS };
    let shown = ci.running.len().min(pipeline_rows);
    let right_edge = WIDTH - MARGIN - PAD;
    let left = MARGIN + PAD;
    let row_y = |row: usize| BODY_TOP + row as f32 * (row_height + GAP);

    if ci.running.is_empty() && queued == 0 {
        empty_message(canvas, strings.t("ci.none"));
        return;
    }

    for (row, pipeline) in ci.running.iter().take(shown).enumerate() {
        let y = row_y(row);
        card(canvas, MARGIN, y, WIDTH - 2.0 * MARGIN, row_height, None);
        let elapsed = view.now - pipeline.started_at;
        let clock_style = VALUE_M.color(match view.ci_tone(elapsed) {
            Tone::Normal => TEXT,
            tone => tone_color(tone),
        });
        let clock_width = canvas.text(
            right_edge,
            y + row_height / 2.0 + Canvas::cap_height(clock_style) / 2.0,
            Align::Right,
            clock_style,
            &stopwatch(elapsed),
        );
        let max = right_edge - clock_width - 24.0 - left;
        let repo = pipeline
            .repo
            .rsplit_once('/')
            .map_or(pipeline.repo.as_str(), |(_, name)| name);
        let (title_baseline, detail_baseline) =
            two_line_baselines(y, row_height, TITLE, DETAIL, 12.0);
        let (fitted, text) = fit(TITLE, MIN_SIZE, max, repo);
        let width = canvas.text(left, title_baseline, Align::Left, fitted, &text);
        let number = format!("#{}", pipeline.number);
        if width + 12.0 + Canvas::measure(LABEL, &number) <= max {
            canvas.text(
                left + width + 12.0,
                title_baseline,
                Align::Left,
                LABEL,
                &number,
            );
        }
        let detail = [pipeline.steps.join(", "), pipeline.branch.clone()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        canvas.text_fit(left, detail_baseline, Align::Left, DETAIL, max, &detail);
    }

    let hidden = ci.running.len() - shown;
    if queued > 0 || hidden > 0 {
        let y = row_y(shown);
        card(canvas, MARGIN, y, WIDTH - 2.0 * MARGIN, row_height, None);
        let baseline = center_baseline(y, row_height, TITLE);
        let mut parts = Vec::new();
        if queued > 0 {
            parts.push(format!("{} {queued}", strings.t("ci.queue")));
            if let Some(oldest) = ci.oldest_queued_secs {
                parts.push(format!(
                    "{} {}",
                    strings.t("ci.waiting"),
                    strings.age(oldest as i64)
                ));
            }
        }
        if hidden > 0 {
            parts.push(strings.tf("ci.more_running", &[("n", &hidden.to_string())]));
        }
        let wait_tone = ci
            .oldest_queued_secs
            .filter(|_| queued > 0)
            .map_or(Tone::Normal, |oldest| view.ci_tone(oldest as i64));
        canvas.text_fit(
            left,
            baseline,
            Align::Left,
            TITLE.color(match wait_tone {
                Tone::Normal => MUTED,
                tone => tone_color(tone),
            }),
            right_edge - left,
            &parts.join(" · "),
        );
    }
}

/// Right edges of the HDD table columns.
const HDD_BUSY_RIGHT: f32 = 340.0;
const HDD_READ_RIGHT: f32 = 570.0;
const HDD_WRITE_RIGHT: f32 = 790.0;
const RIGHT_EDGE: f32 = WIDTH - MARGIN - PAD;

/// Rows of equal height filling `top..HEIGHT - MARGIN`.
fn rows(top: f32, count: usize, gap: f32) -> (f32, impl Fn(usize) -> f32) {
    let count = count.max(1);
    let height = (HEIGHT - MARGIN - top - gap * (count as f32 - 1.0)) / count as f32;
    (height, move |index: usize| {
        top + index as f32 * (height + gap)
    })
}

fn row_panel(canvas: &mut Canvas, y: f32, height: f32) {
    canvas.rounded_rect(MARGIN, y, WIDTH - 2.0 * MARGIN, height, 10.0, PANEL);
}

fn table_header(canvas: &mut Canvas, columns: &[(f32, Align, &str)]) -> f32 {
    let style = LABEL.size(MIN_SIZE);
    let baseline = BODY_TOP + 2.0 + Canvas::cap_height(style);
    for (x, align, text) in columns {
        canvas.text(*x, baseline, *align, style, text);
    }
    baseline + 12.0
}

/// One row per HDD: busy %, read, write, temperature. Scales to any count.
fn hdd(canvas: &mut Canvas, view: &View) {
    let strings = view.strings;
    let thresholds = &view.config.thresholds;
    let disks: Vec<_> = view
        .snapshot
        .disks
        .iter()
        .filter(|disk| disk.kind == DiskKind::Hdd)
        .collect();
    if disks.is_empty() {
        empty_message(canvas, strings.t("hdd.none"));
        return;
    }
    let top = table_header(
        canvas,
        &[
            (MARGIN + PAD, Align::Left, strings.t("hdd.disk")),
            (HDD_BUSY_RIGHT, Align::Right, strings.t("hdd.busy")),
            (HDD_READ_RIGHT, Align::Right, strings.t("hdd.read")),
            (HDD_WRITE_RIGHT, Align::Right, strings.t("hdd.write")),
            (RIGHT_EDGE, Align::Right, "°C"),
        ],
    );
    let (height, row_y) = rows(top, disks.len(), 4.0);
    let size = (height * 0.75).clamp(MIN_SIZE, 60.0);
    for (index, disk) in disks.iter().enumerate() {
        let y = row_y(index);
        row_panel(canvas, y, height);
        let baseline = y + height / 2.0 + Canvas::cap_height(TITLE.size(size)) / 2.0;
        let bold = TITLE.size(size);
        let plain = DETAIL.color(TEXT).size(size);
        let broken = !disk.present || disk.smart_failed;
        canvas.text(
            MARGIN + PAD,
            baseline,
            Align::Left,
            bold.color(if broken {
                RED
            } else if disk.self_test.is_some() {
                AMBER
            } else {
                TEXT
            }),
            &disk.label,
        );
        if !disk.present {
            canvas.text(
                HDD_BUSY_RIGHT,
                baseline,
                Align::Right,
                bold.color(RED),
                strings.t("hdd.missing"),
            );
            continue;
        }
        let busy_tone = thresholds
            .hdd_busy_percent
            .classify(disk.busy_percent)
            .max_warning();
        canvas.text(
            HDD_BUSY_RIGHT,
            baseline,
            Align::Right,
            bold.color(tone_color(busy_tone)),
            &format!("{:.0}%", disk.busy_percent),
        );
        canvas.text(
            HDD_READ_RIGHT,
            baseline,
            Align::Right,
            plain,
            &megabytes(disk.read_bps),
        );
        canvas.text(
            HDD_WRITE_RIGHT,
            baseline,
            Align::Right,
            plain,
            &megabytes(disk.write_bps),
        );
        let (last, color) = match (&disk.self_test, disk.smart_failed) {
            (_, true) => ("SMART!".to_owned(), RED),
            (Some(test), _) => (
                test.remaining_percent.map_or_else(
                    || strings.t("hdd.self_test").to_owned(),
                    |left| format!("{} {left}%", strings.t("hdd.self_test")),
                ),
                AMBER,
            ),
            (None, _) => (
                temperature(disk.temp_c),
                tone_color(view.tone(disk.temp_c, thresholds.hdd_temp_c)),
            ),
        };
        canvas.text(RIGHT_EDGE, baseline, Align::Right, bold.color(color), &last);
    }
}

/// Pools (health, space, scan) and the busiest datasets with direction.
fn zfs(canvas: &mut Canvas, view: &View) {
    let strings = view.strings;
    let snapshot = view.snapshot;
    let pool_height = 56.0;
    let mut y = BODY_TOP;
    for pool in &snapshot.pools {
        row_panel(canvas, y, pool_height);
        let baseline = y + pool_height / 2.0 + Canvas::cap_height(TITLE) / 2.0;
        let name_width = canvas.text(MARGIN + PAD, baseline, Align::Left, TITLE, &pool.name);
        let healthy = pool.health == "ONLINE" && pool.errors == 0 && pool.bad_devices.is_empty();
        let mut parts = vec![pool.health.clone()];
        if pool.total > 0 {
            parts.push(format!(
                "{} {} {} {}",
                strings.t("zfs.free_of"),
                strings.bytes(pool.total.saturating_sub(pool.used)),
                strings.t("home.of"),
                strings.bytes(pool.total)
            ));
        }
        let (scan_text, color) = match &pool.scan {
            Some(scan) => {
                let mut text = format!("{} {:.0}%", scan_name(scan.kind), scan.percent);
                if scan.paused {
                    text.push_str(&format!(" {}", strings.t("home.paused")));
                } else if let Some(eta) = scan.eta_secs {
                    text.push_str(&format!(
                        " {} {}",
                        strings.t("zfs.eta"),
                        strings.age(eta as i64)
                    ));
                }
                (Some(text), AMBER)
            }
            None => (None, if healthy { GREEN } else { RED }),
        };
        parts.extend(scan_text);
        let left = MARGIN + PAD + name_width + 24.0;
        canvas.text_fit(
            left,
            baseline,
            Align::Left,
            DETAIL.color(color),
            RIGHT_EDGE - left,
            &parts.join(" · "),
        );
        y += pool_height + GAP;
    }

    let panel = Tile {
        x: MARGIN,
        y,
        w: WIDTH - 2.0 * MARGIN,
        h: HEIGHT - MARGIN - y,
    };
    if panel.h < 60.0 {
        return;
    }
    panel.panel(canvas);
    let header_baseline = panel.y + 10.0 + Canvas::cap_height(LABEL);
    canvas.text(
        MARGIN + PAD,
        header_baseline,
        Align::Left,
        LABEL,
        strings.t("zfs.activity"),
    );
    let busy: Vec<_> = snapshot
        .datasets
        .iter()
        .filter(|dataset| megabytes(dataset.read_bps.saturating_add(dataset.write_bps)) != "0")
        .collect();
    let line_step = Canvas::cap_height(DETAIL) + 14.0;
    let capacity = ((panel.y + panel.h - 8.0 - header_baseline) / line_step).floor() as usize;
    if busy.is_empty() {
        canvas.text(
            MARGIN + PAD,
            header_baseline + line_step,
            Align::Left,
            DETAIL,
            strings.t("zfs.datasets_idle"),
        );
        return;
    }
    for (index, dataset) in busy.iter().take(capacity).enumerate() {
        let baseline = header_baseline + line_step * (index as f32 + 1.0);
        let value = read_write(strings, dataset.read_bps, dataset.write_bps);
        let value_width = canvas.text(
            RIGHT_EDGE,
            baseline,
            Align::Right,
            DETAIL.color(TEXT),
            &value,
        );
        canvas.text_fit(
            MARGIN + PAD,
            baseline,
            Align::Left,
            DETAIL.color(TEXT),
            RIGHT_EDGE - value_width - 24.0 - MARGIN - PAD,
            &dataset.name,
        );
    }
}

/// Compact tiles: label left, value right; four columns, up to three rows.
fn temps(canvas: &mut Canvas, view: &View) {
    let strings = view.strings;
    let snapshot = view.snapshot;
    let host = &snapshot.host;
    let thresholds = &view.config.thresholds;
    let gpu_temp = host.gpu.as_ref().and_then(|gpu| gpu.temp_c);
    let mut items: Vec<(String, String, Tone)> = vec![
        (
            "CPU".to_owned(),
            temperature(host.cpu_temp_c),
            view.tone(host.cpu_temp_c, thresholds.cpu_temp_c),
        ),
        (
            "GPU".to_owned(),
            temperature(gpu_temp),
            view.tone(gpu_temp, thresholds.gpu_temp_c),
        ),
        (
            strings.t("temps.ram").to_owned(),
            temperature(host.ram_temp_c),
            view.tone(host.ram_temp_c, thresholds.ram_temp_c),
        ),
        (
            strings.t("temps.fan").to_owned(),
            host.fans
                .iter()
                .map(|fan| fan.rpm)
                .max()
                .map_or_else(|| "--".to_owned(), |rpm| rpm.to_string()),
            Tone::Normal,
        ),
    ];
    for disk in &snapshot.disks {
        let level = match disk.kind {
            DiskKind::Hdd => thresholds.hdd_temp_c,
            DiskKind::Nvme => thresholds.nvme_temp_c,
        };
        let (value, tone) = if disk.present {
            (temperature(disk.temp_c), view.tone(disk.temp_c, level))
        } else {
            (strings.t("temps.gone").to_owned(), Tone::Critical)
        };
        items.push((disk.label.clone(), value, tone));
    }
    let columns = 4;
    let row_count = items.len().div_ceil(columns).clamp(1, 3);
    let tiles = Tile::grid(columns, row_count, BODY_TOP, BODY_HEIGHT);
    let value_size = if row_count == 3 { 64.0 } else { 84.0 };
    for (tile, (label, value, tone)) in tiles.iter().zip(&items) {
        tile.panel(canvas);
        let value_style = VALUE_L.size(value_size).color(tone_color(*tone));
        let baseline = tile.y + tile.h / 2.0 + Canvas::cap_height(value_style) / 2.0;
        let value_width = canvas.text(
            tile.x + tile.w - PAD,
            baseline,
            Align::Right,
            value_style,
            value,
        );
        canvas.text_fit(
            tile.x + PAD,
            tile.y + tile.h / 2.0 + Canvas::cap_height(LABEL) / 2.0,
            Align::Left,
            LABEL,
            tile.inner() - value_width - 8.0,
            label,
        );
    }
}

/// Full-screen blinking alarm: one big headline and every broken thing.
fn alarm(view: &View) -> Canvas {
    let items = super::alarms(view.snapshot, view.strings);
    let (background, foreground) = if view.flash {
        (ALARM_RED, TEXT)
    } else {
        (BACKGROUND, RED)
    };
    let mut canvas = Canvas::new(WIDTH as u32, HEIGHT as u32, background);
    let title = Style::new(112.0, Weight::Bold, foreground);
    let what = Style::new(100.0, Weight::Bold, foreground);
    let why = Style::new(72.0, Weight::Bold, foreground);
    // Headline plus the item block form one group; center it vertically so a
    // single broken thing does not hug the top edge.
    let gap_title = 24.0;
    let gap_item = 24.0;
    let block = Canvas::cap_height(title)
        + gap_title
        + Canvas::cap_height(what)
        + gap_item
        + Canvas::cap_height(why);
    let top = (HEIGHT - block) / 2.0;
    let title_baseline = top + Canvas::cap_height(title);
    canvas.text(
        WIDTH / 2.0,
        title_baseline,
        Align::Center,
        title,
        view.strings.t("alarm.title"),
    );

    let area_height = HEIGHT - MARGIN - top;
    let inner = WIDTH - 2.0 * PAD;
    match items.as_slice() {
        [] => {}
        [only] => {
            let what_baseline =
                top + Canvas::cap_height(title) + gap_title + Canvas::cap_height(what);
            canvas.text_fit(
                WIDTH / 2.0,
                what_baseline,
                Align::Center,
                what,
                inner,
                &only.what,
            );
            canvas.text_fit(
                WIDTH / 2.0,
                what_baseline + gap_item + Canvas::cap_height(why),
                Align::Center,
                why,
                inner,
                &only.why,
            );
        }
        many => {
            let columns = if many.len() <= 3 {
                1
            } else if many.len() <= 8 {
                2
            } else {
                3
            };
            let max_rows = 4;
            let slots = columns * max_rows;
            let mut lines: Vec<String> = many
                .iter()
                .map(|item| format!("{}: {}", item.what, item.why))
                .collect();
            if lines.len() > slots {
                let hidden = lines.len() - (slots - 1);
                lines.truncate(slots - 1);
                lines.push(format!("и ещё {hidden}"));
            }
            let row_count = lines.len().div_ceil(columns);
            let cell_height = area_height / row_count as f32;
            let cell_width = (WIDTH - 2.0 * PAD) / columns as f32;
            // One common size: the largest at which every line fits its cell.
            let start = (cell_height * 0.72).clamp(MIN_SIZE, 72.0);
            let size = lines
                .iter()
                .map(|line| {
                    fit(
                        Style::new(start, Weight::Bold, foreground),
                        MIN_SIZE,
                        cell_width - 16.0,
                        line,
                    )
                    .0
                    .size
                })
                .fold(start, f32::min);
            let style = Style::new(size, Weight::Bold, foreground);
            for (index, line) in lines.iter().enumerate() {
                let column = index / row_count;
                let row = index % row_count;
                let x = PAD + column as f32 * cell_width;
                let baseline = top
                    + row as f32 * cell_height
                    + cell_height / 2.0
                    + Canvas::cap_height(style) / 2.0;
                canvas.text_fit(x, baseline, Align::Left, style, cell_width - 16.0, line);
            }
        }
    }
    canvas
}

// ------------------------------------------------- boot and power transitions

/// Headline panel of the boot and shutdown pages.
struct PhaseHead<'a> {
    title: &'a str,
    accent: Rgb,
    big: String,
    big_label: String,
    /// 0..=1 meter under the big value, when the total is known.
    progress: Option<f32>,
    details: Vec<String>,
}

/// One row of the service list.
struct PhaseRow {
    text: String,
    color: Rgb,
}

/// Shared layout: headline panel on the left, the interesting services on
/// the right. No status bar — the phase owns the whole screen.
fn phase_frame(
    canvas: &mut Canvas,
    head: &PhaseHead,
    rows: &[PhaseRow],
    strings: &I18n,
    empty: &str,
) {
    let top = MARGIN;
    let height = HEIGHT - 2.0 * MARGIN;
    let panel_w = 356.0;
    Tile {
        x: MARGIN,
        y: top,
        w: panel_w,
        h: height,
    }
    .panel(canvas);
    let x = MARGIN + PAD;
    let title_style = Style::new(54.0, Weight::Bold, head.accent);
    let title_baseline = top + 24.0 + Canvas::cap_height(title_style);
    canvas.text_fit(
        x,
        title_baseline,
        Align::Left,
        title_style,
        panel_w - 2.0 * PAD,
        head.title,
    );
    let (fitted, big) = fit(VALUE_L, 44.0, panel_w - 2.0 * PAD, &head.big);
    let big_baseline = title_baseline + 34.0 + Canvas::cap_height(VALUE_L);
    canvas.text(x - 3.0, big_baseline, Align::Left, fitted, &big);
    let label_baseline = big_baseline + 18.0 + Canvas::cap_height(LABEL);
    canvas.text(x, label_baseline, Align::Left, LABEL, &head.big_label);
    if let Some(progress) = head.progress {
        canvas.meter(
            Area::new(x, label_baseline + 16.0, panel_w - 2.0 * PAD, 12.0),
            progress,
            head.accent,
            TRACK,
        );
    }
    // Clear the meter (16 px below the label + 12 px bar) with an 18 px
    // visual gap before the first detail line; two lines fit the panel.
    let detail_style = DETAIL.color(TEXT).size(40.0);
    let mut baseline = label_baseline + 46.0 + Canvas::cap_height(detail_style);
    for detail in head.details.iter().take(2) {
        canvas.text_fit(
            x,
            baseline,
            Align::Left,
            detail_style,
            panel_w - 2.0 * PAD,
            detail,
        );
        baseline += Canvas::cap_height(detail_style) + 16.0;
    }

    let list_x = MARGIN + panel_w + GAP;
    let list_w = WIDTH - MARGIN - list_x;
    if rows.is_empty() {
        let style = TITLE.color(MUTED);
        canvas.text(
            list_x + list_w / 2.0,
            top + height / 2.0 + Canvas::cap_height(style) / 2.0,
            Align::Center,
            style,
            empty,
        );
        return;
    }
    const MAX_ROWS: usize = 6;
    let overflow = rows.len() > MAX_ROWS;
    let shown = if overflow { MAX_ROWS - 1 } else { rows.len() };
    // With overflow the last slot belongs to the "и ещё N" row.
    let slots = if overflow { MAX_ROWS } else { shown };
    let row_h = (height - GAP * (slots as f32 - 1.0)) / slots as f32;
    let size = (row_h * 0.55).clamp(MIN_SIZE, 46.0);
    for (row, item) in rows.iter().take(shown).enumerate() {
        let y = top + row as f32 * (row_h + GAP);
        card(canvas, list_x, y, list_w, row_h, None);
        let baseline = center_baseline(y, row_h, TITLE.size(size));
        canvas.text_fit(
            list_x + PAD,
            baseline,
            Align::Left,
            TITLE.size(size).color(item.color),
            list_w - 2.0 * PAD,
            &item.text,
        );
    }
    if overflow {
        let y = top + shown as f32 * (row_h + GAP);
        card(canvas, list_x, y, list_w, row_h, None);
        let baseline = center_baseline(y, row_h, TITLE.size(size));
        canvas.text_fit(
            list_x + PAD,
            baseline,
            Align::Left,
            TITLE.size(size).color(MUTED),
            list_w - 2.0 * PAD,
            &strings.tf("alarm.more", &[("n", &(rows.len() - shown).to_string())]),
        );
    }
}

/// Boot page: the watched services that are not up yet.
fn startup(view: &View) -> Canvas {
    let strings = view.strings;
    let pending = super::startup_pending(view.snapshot);
    let total = view.snapshot.services.len();
    let up = total - pending.len();
    let head = PhaseHead {
        title: strings.t("phase.startup"),
        accent: BLUE,
        big: format!("{up} / {total}"),
        big_label: strings.tn("plurals.services_up", up as u64),
        progress: (total > 0).then(|| up as f32 / total as f32),
        details: vec![match view.snapshot.host.uptime_secs {
            Some(uptime) => format!("{} {}", strings.t("phase.uptime"), stopwatch(uptime as i64)),
            None => format!("{} --", strings.t("phase.uptime")),
        }],
    };
    let rows: Vec<PhaseRow> = pending
        .iter()
        .map(|subject| PhaseRow {
            text: (*subject).to_owned(),
            color: TEXT,
        })
        .collect();
    let mut canvas = Canvas::new(WIDTH as u32, HEIGHT as u32, BACKGROUND);
    phase_frame(
        &mut canvas,
        &head,
        &rows,
        strings,
        strings.t("phase.all_up"),
    );
    canvas
}

/// Shutdown/reboot page, driven by `ExecStopPost` while systemd stops units.
/// Failed watched services first in red, then the ones still stopping in
/// amber; green headline once every watched service is down.
pub fn shutdown_page(
    target: Target,
    strings: &I18n,
    progress: &Progress,
    have_state: bool,
    containers: Option<u32>,
    elapsed_secs: u64,
    done: bool,
) -> RgbImage {
    let mut rows: Vec<PhaseRow> = progress
        .failed
        .iter()
        .map(|subject| PhaseRow {
            text: strings.subject(subject).to_owned(),
            color: RED,
        })
        .collect();
    rows.extend(progress.pending.iter().map(|subject| PhaseRow {
        text: subject.clone(),
        color: AMBER,
    }));
    let head = PhaseHead {
        title: strings.target_title(target),
        accent: if done { GREEN } else { AMBER },
        big: if have_state {
            format!("{} / {}", progress.stopped, progress.total)
        } else {
            "--".to_owned()
        },
        big_label: strings.tn("plurals.services_stopped", progress.stopped as u64),
        progress: (have_state && progress.total > 0)
            .then(|| progress.stopped as f32 / progress.total as f32),
        details: {
            let mut details = vec![format!(
                "{} {}",
                strings.t("phase.elapsed"),
                stopwatch(elapsed_secs as i64)
            )];
            if let Some(containers) = containers {
                details.push(strings.tn("plurals.containers", containers as u64));
            }
            details
        },
    };
    let mut canvas = Canvas::new(WIDTH as u32, HEIGHT as u32, BACKGROUND);
    let empty = if done {
        strings.t("phase.all_stopped")
    } else {
        strings.t("phase.status_unavailable")
    };
    phase_frame(&mut canvas, &head, &rows, strings, empty);
    canvas.into_rgb()
}

fn load(canvas: &mut Canvas, view: &View) {
    let strings = view.strings;
    let snapshot = view.snapshot;
    let host = &snapshot.host;
    let tiles = Tile::grid(2, 1, BODY_TOP, BODY_HEIGHT);
    let total_cores = format!("CPU · {}", percent(host.cpu_percent));
    let cpu_rows: Vec<(String, String)> = snapshot
        .top_cpu
        .iter()
        .map(|item| (item.name.clone(), format!("{:.0}%", item.percent)))
        .collect();
    list_panel(canvas, &tiles[0], &total_cores, strings, &cpu_rows);

    let memory_header = format!(
        "{} {}",
        strings.t("load.memory"),
        strings.bytes(host.mem_available)
    );
    let memory_rows: Vec<(String, String)> = snapshot
        .top_memory
        .iter()
        .map(|item| (item.name.clone(), strings.bytes(item.bytes)))
        .collect();
    list_panel(canvas, &tiles[1], &memory_header, strings, &memory_rows);
}

fn list_panel(
    canvas: &mut Canvas,
    tile: &Tile,
    header: &str,
    strings: &I18n,
    rows: &[(String, String)],
) {
    tile.panel(canvas);
    let x = tile.x + PAD;
    let right = tile.x + tile.w - PAD;
    let header_baseline = tile.y + PAD + Canvas::cap_height(LABEL);
    canvas.text_fit(x, header_baseline, Align::Left, LABEL, tile.inner(), header);
    if rows.is_empty() {
        canvas.text(
            x,
            header_baseline + 60.0,
            Align::Left,
            DETAIL,
            strings.t("load.no_data"),
        );
        return;
    }
    let row_style = LABEL.color(TEXT).size(42.0);
    let name_style = DETAIL.color(TEXT).size(42.0);
    let step = (tile.h - (header_baseline - tile.y) - PAD) / 5.0;
    for (index, (name, value)) in rows.iter().take(5).enumerate() {
        let baseline = header_baseline + step * (index as f32 + 1.0);
        let value_width = canvas.text(right, baseline, Align::Right, row_style, value);
        canvas.text_fit(
            x,
            baseline,
            Align::Left,
            name_style,
            right - value_width - 16.0 - x,
            name,
        );
    }
}

/// One row per volume and pool: free space, total, a meter and used %.
fn space(canvas: &mut Canvas, view: &View) {
    let strings = view.strings;
    let snapshot = view.snapshot;
    let level = view.config.thresholds.space_used_percent;
    // (label, used, total, available, problem)
    let mut items: Vec<(String, u64, u64, bool, bool)> = snapshot
        .volumes
        .iter()
        .map(|volume| {
            (
                volume.label.clone(),
                volume.used,
                volume.total,
                volume.mounted,
                false,
            )
        })
        .collect();
    for pool in &snapshot.pools {
        let problem = pool.health != "ONLINE" || pool.errors > 0 || !pool.bad_devices.is_empty();
        items.push((
            pool.name.clone(),
            pool.used,
            pool.total,
            pool.total > 0,
            problem,
        ));
    }
    if items.is_empty() {
        empty_message(canvas, strings.t("space.none"));
        return;
    }
    const FREE_RIGHT: f32 = 420.0;
    const TOTAL_LEFT: f32 = 440.0;
    const METER_LEFT: f32 = 640.0;
    const METER_RIGHT: f32 = 820.0;
    let (height, row_y) = rows(BODY_TOP, items.len(), 6.0);
    let size = (height * 0.7).clamp(MIN_SIZE, 56.0);
    let bold = TITLE.size(size);
    let plain = DETAIL.size(size);
    for (index, (label, used, total, available, problem)) in items.iter().enumerate() {
        let y = row_y(index);
        row_panel(canvas, y, height);
        let baseline = y + height / 2.0 + Canvas::cap_height(bold) / 2.0;
        canvas.text_fit(
            MARGIN + PAD,
            baseline,
            Align::Left,
            LABEL.size(size),
            FREE_RIGHT - 160.0 - MARGIN - PAD,
            label,
        );
        if !*available {
            canvas.text(
                FREE_RIGHT,
                baseline,
                Align::Right,
                bold.color(RED),
                strings.t("temps.gone"),
            );
            continue;
        }
        let used_percent = (*total > 0).then(|| *used as f32 * 100.0 / *total as f32);
        let tone = if *problem {
            Tone::Critical
        } else {
            view.tone(used_percent, level)
        };
        canvas.text(
            FREE_RIGHT,
            baseline,
            Align::Right,
            bold.color(tone_color(tone)),
            &strings.bytes(total.saturating_sub(*used)),
        );
        canvas.text(
            TOTAL_LEFT,
            baseline,
            Align::Left,
            plain,
            &format!("{} {}", strings.t("home.of"), strings.bytes(*total)),
        );
        if let Some(used) = used_percent {
            canvas.meter(
                Area::new(
                    METER_LEFT,
                    y + height / 2.0 - 6.0,
                    METER_RIGHT - METER_LEFT,
                    12.0,
                ),
                used / 100.0,
                tone_fill(level.classify(used)),
                TRACK,
            );
            canvas.text(
                RIGHT_EDGE,
                baseline,
                Align::Right,
                plain.color(TEXT),
                &format!("{used:.0}%"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::parse(include_str!("../../../../config.example.toml")).unwrap()
    }
    #[test]
    fn phase_pages_render_at_display_size() {
        let progress = Progress {
            total: 13,
            stopped: 5,
            pending: vec!["immich".into(), "gitea".into()],
            failed: vec!["docker".into()],
        };
        let frame = shutdown_page(
            Target::Reboot,
            &I18n::english(),
            &progress,
            true,
            Some(9),
            41,
            false,
        );
        assert_eq!(frame.dimensions(), asterctl_lcd::DISPLAY_SIZE);

        let done = shutdown_page(
            Target::Poweroff,
            &I18n::russian(),
            &Progress::default(),
            true,
            Some(0),
            55,
            true,
        );
        assert_eq!(done.dimensions(), asterctl_lcd::DISPLAY_SIZE);

        // Overflow of the service list.
        let many = Progress {
            total: 13,
            stopped: 2,
            pending: (0..8).map(|index| format!("svc{index}")).collect(),
            failed: Vec::new(),
        };
        let frame = shutdown_page(
            Target::Reboot,
            &I18n::english(),
            &many,
            true,
            None,
            10,
            false,
        );
        assert_eq!(frame.dimensions(), asterctl_lcd::DISPLAY_SIZE);
    }

    #[test]
    fn startup_page_renders_at_display_size() {
        let config = config();
        let mut snapshot = Snapshot::default();
        snapshot.host.uptime_secs = Some(47);
        snapshot.services = (0..8)
            .map(|index| crate::model::ServiceStatus {
                subject: format!("svc{index}"),
                active: index == 7,
            })
            .collect();
        let frame = render(
            Page::Startup,
            &snapshot,
            &config,
            0,
            (0, 1),
            true,
            &I18n::english(),
        );
        assert_eq!(frame.dimensions(), asterctl_lcd::DISPLAY_SIZE);
    }

    #[test]
    fn clock_applies_offset_and_wraps_midnight() {
        assert_eq!(clock_text(86_399, 0), "23:59");
        assert_eq!(clock_text(22 * 3600, 180), "01:00");
    }

    #[test]
    fn dataset_traffic_names_its_direction() {
        const MB: u64 = 1024 * 1024;
        let s = I18n::russian();
        assert_eq!(read_write(&s, 76 * MB, 0), "чтение 76");
        assert_eq!(read_write(&s, 10_000, 120 * MB), "запись 120");
        assert_eq!(read_write(&s, 5 * MB, 3 * MB), "чтение 5, запись 3");
        assert_eq!(read_write(&I18n::english(), 76 * MB, 0), "read 76");
    }

    #[test]
    fn units_split_from_formatted_sizes() {
        assert_eq!(I18n::russian().split_unit("68 ГБ"), ("68".to_owned(), "ГБ"));
        assert_eq!(
            I18n::russian().split_unit("6.8 ТБ"),
            ("6.8".to_owned(), "ТБ")
        );
        assert_eq!(
            I18n::english().split_unit("6.8 TB"),
            ("6.8".to_owned(), "TB")
        );
    }
}
