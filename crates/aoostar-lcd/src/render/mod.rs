//! Page selection (rotation) and rendering.

pub mod canvas;
pub mod i18n;
mod pages;
pub mod theme;

use image::RgbImage;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::model::{Severity, Snapshot};

use i18n::Strings;

pub use pages::{render, shutdown_page};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Full-screen blinking takeover: a disk failed or a critical service is
    /// down without a maintenance flag. Replaces every other page.
    Alarm,
    /// Boot window: watched services that are not up yet. Replaces every
    /// other page until they are or the window ends; hardware alarms win.
    Startup,
    /// CPU / memory / GPU / HDD summary; always shown, twice as long.
    Home,
    /// Outages and firing alerts; only while there are any.
    Problems,
    /// Running and queued pipelines; only while CI is active.
    Ci,
    /// Per-HDD table: busy %, read, write, temperature; only while disks work.
    Hdd,
    /// Pools (health, scan) and the busiest datasets; shown with the HDD page.
    Zfs,
    Temps,
    Load,
    Space,
}

impl Page {
    pub const ALL: [Page; 10] = [
        Page::Alarm,
        Page::Startup,
        Page::Home,
        Page::Problems,
        Page::Ci,
        Page::Hdd,
        Page::Zfs,
        Page::Temps,
        Page::Load,
        Page::Space,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Page::Alarm => "alarm",
            Page::Startup => "startup",
            Page::Home => "home",
            Page::Problems => "problems",
            Page::Ci => "ci",
            Page::Hdd => "hdd",
            Page::Zfs => "zfs",
            Page::Temps => "temps",
            Page::Load => "load",
            Page::Space => "space",
        }
    }
}

/// One line of the alarm screen: what is broken and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlarmItem {
    pub what: String,
    pub why: String,
}

/// One line per broken thing; several reasons for the same thing are joined.
fn push_alarm(items: &mut Vec<AlarmItem>, what: &str, why: String) {
    match items.iter_mut().find(|item| item.what == what) {
        Some(item) => {
            if !item.why.split(", ").any(|known| known == why) {
                item.why.push_str(", ");
                item.why.push_str(&why);
            }
        }
        None => items.push(AlarmItem {
            what: what.to_owned(),
            why,
        }),
    }
}

/// Hardware alarms: a configured disk missing or SMART-failed, a pool leaf
/// device not `ONLINE`, a pool not `ONLINE`. True in every phase, including
/// the boot window.
pub fn hardware_alarms(snapshot: &Snapshot, strings: &Strings) -> Vec<AlarmItem> {
    let mut items: Vec<AlarmItem> = Vec::new();
    for disk in &snapshot.disks {
        if !disk.present {
            push_alarm(&mut items, &disk.label, strings.alarm_missing.to_owned());
        } else if disk.smart_failed {
            push_alarm(
                &mut items,
                &disk.label,
                strings.alarm_smart_failed.to_owned(),
            );
        }
    }
    for pool in &snapshot.pools {
        for device in &pool.bad_devices {
            push_alarm(&mut items, &device.name, device.state.clone());
        }
        if pool.health != "ONLINE" {
            push_alarm(&mut items, &pool.name, pool.health.clone());
        }
    }
    items
}

/// Critical services down without a maintenance flag. Expected while the
/// system is booting, so the startup page suppresses them until it ends.
pub fn service_alarms(snapshot: &Snapshot, strings: &Strings) -> Vec<AlarmItem> {
    let mut items: Vec<AlarmItem> = Vec::new();
    let pool_names: Vec<&str> = snapshot
        .pools
        .iter()
        .map(|pool| pool.name.as_str())
        .collect();
    for problem in snapshot.health.active_outages() {
        if problem.severity == Severity::Critical && !pool_names.contains(&problem.subject.as_str())
        {
            push_alarm(
                &mut items,
                &strings.subject(&problem.subject),
                strings.alarm_service_down.to_owned(),
            );
        }
    }
    items
}

/// Every alarm line: hardware first, then services.
pub fn alarms(snapshot: &Snapshot, strings: &Strings) -> Vec<AlarmItem> {
    let mut items = hardware_alarms(snapshot, strings);
    items.extend(service_alarms(snapshot, strings));
    items
}

/// Watched services systemd does not report active.
pub fn startup_pending(snapshot: &Snapshot) -> Vec<&str> {
    snapshot
        .services
        .iter()
        .filter(|service| !service.active)
        .map(|service| service.subject.as_str())
        .collect()
}

/// The boot window: recently booted and watched services still coming up.
pub fn startup_relevant(snapshot: &Snapshot, config: &Config) -> bool {
    matches!(
        snapshot.host.uptime_secs,
        Some(uptime) if uptime < config.services.startup_window_secs
    ) && !startup_pending(snapshot).is_empty()
}

pub fn hdd_relevant(snapshot: &Snapshot, config: &Config) -> bool {
    snapshot.hdd_busy_max() >= config.thresholds.hdd_busy_percent.0
        || snapshot
            .pools
            .iter()
            .any(|pool| pool.scan.as_ref().is_some_and(|scan| !scan.paused))
        || snapshot.hdd_self_test_running()
}

pub fn ci_relevant(snapshot: &Snapshot) -> bool {
    !snapshot.ci.running.is_empty() || snapshot.ci.queued.unwrap_or(0) > 0
}

/// Pages for the current state. An alarm replaces everything; otherwise
/// contextual pages follow Home, then the calm detail pages.
pub fn plan(snapshot: &Snapshot, config: &Config) -> Vec<Page> {
    let strings = i18n::Strings::get(config.display.language);
    // Broken hardware is true in every phase. Services that are not up yet
    // are what booting looks like, so the startup page owns them.
    if !hardware_alarms(snapshot, strings).is_empty() {
        return vec![Page::Alarm];
    }
    if startup_relevant(snapshot, config) {
        return vec![Page::Startup];
    }
    if !service_alarms(snapshot, strings).is_empty() {
        return vec![Page::Alarm];
    }
    let mut pages = vec![Page::Home];
    if !snapshot.health.problems.is_empty() {
        pages.push(Page::Problems);
    }
    if ci_relevant(snapshot) {
        pages.push(Page::Ci);
    }
    if hdd_relevant(snapshot, config) {
        pages.extend([Page::Hdd, Page::Zfs]);
    }
    pages.extend([Page::Temps, Page::Load, Page::Space]);
    pages
}

pub struct Rotation {
    plan: Vec<Page>,
    index: usize,
    shown_at: Instant,
}

impl Rotation {
    pub fn new(now: Instant) -> Self {
        Self {
            plan: vec![Page::Home],
            index: 0,
            shown_at: now,
        }
    }

    /// Current page plus its 0-based position and the plan length.
    pub fn tick(
        &mut self,
        snapshot: &Snapshot,
        config: &Config,
        now: Instant,
    ) -> (Page, usize, usize) {
        let current = self.plan[self.index];
        let next_plan = plan(snapshot, config);
        if next_plan != self.plan {
            // Keep showing the same page if it is still relevant.
            self.index = next_plan
                .iter()
                .position(|page| *page == current)
                .unwrap_or(0);
            if next_plan[self.index] != current {
                self.shown_at = now;
            }
            self.plan = next_plan;
        }
        let page = self.plan[self.index];
        if now.duration_since(self.shown_at) >= dwell(page, config) {
            self.index = (self.index + 1) % self.plan.len();
            self.shown_at = now;
        }
        (self.plan[self.index], self.index, self.plan.len())
    }
}

fn dwell(page: Page, config: &Config) -> Duration {
    let base = Duration::from_secs(config.display.page_seconds);
    if page == Page::Home { base * 2 } else { base }
}

/// Renders every page of a snapshot (offline preview).
pub fn render_all(snapshot: &Snapshot, config: &Config, now_unix: i64) -> Vec<(Page, RgbImage)> {
    Page::ALL
        .iter()
        .enumerate()
        .map(|(index, page)| {
            (
                *page,
                render(
                    *page,
                    snapshot,
                    config,
                    now_unix,
                    (index, Page::ALL.len()),
                    true,
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BadDevice, Disk, Pipeline, Pool, Problem, ProblemClass, ServiceStatus};

    fn config() -> Config {
        Config::parse(include_str!("../../../../granite/aoostar-lcd.toml")).unwrap()
    }

    fn test_strings() -> &'static Strings {
        Strings::get(config().display.language)
    }

    fn outage(subject: &str, severity: Severity) -> Problem {
        Problem {
            class: ProblemClass::Outage,
            severity,
            subject: subject.to_owned(),
            ..Problem::default()
        }
    }

    #[test]
    fn calm_state_shows_only_summary_and_detail_pages() {
        let pages = plan(&Snapshot::default(), &config());
        assert_eq!(pages, [Page::Home, Page::Temps, Page::Load, Page::Space]);
    }

    #[test]
    fn running_ci_and_alerts_add_contextual_pages_after_home() {
        let mut snapshot = Snapshot::default();
        snapshot.ci.running.push(Pipeline::default());
        snapshot.health.problems.push(Problem::default());
        let pages = plan(&snapshot, &config());
        assert_eq!(&pages[..3], [Page::Home, Page::Problems, Page::Ci]);
    }

    #[test]
    fn only_unplanned_critical_outage_raises_the_alarm() {
        let mut snapshot = Snapshot::default();
        snapshot
            .health
            .problems
            .push(outage("urbackup", Severity::Maintenance));
        snapshot
            .health
            .problems
            .push(outage("cars", Severity::Warning));
        assert!(alarms(&snapshot, test_strings()).is_empty());
        assert_ne!(plan(&snapshot, &config()), [Page::Alarm]);

        snapshot
            .health
            .problems
            .push(outage("immich", Severity::Critical));
        assert_eq!(
            alarms(&snapshot, test_strings()),
            [AlarmItem {
                what: "immich".into(),
                why: "лежит".into()
            }]
        );
        assert_eq!(plan(&snapshot, &config()), [Page::Alarm]);
    }

    #[test]
    fn boot_window_shows_startup_page_not_service_alarm() {
        let mut snapshot = Snapshot::default();
        snapshot.host.uptime_secs = Some(90);
        snapshot.services = vec![
            ServiceStatus {
                subject: "gitea".into(),
                active: true,
            },
            ServiceStatus {
                subject: "immich".into(),
                active: false,
            },
        ];
        assert_eq!(startup_pending(&snapshot), ["immich"]);
        assert!(startup_relevant(&snapshot, &config()));
        assert_eq!(plan(&snapshot, &config()), [Page::Startup]);
    }

    #[test]
    fn boot_window_suppresses_service_alarm_but_not_hardware() {
        let mut snapshot = Snapshot::default();
        snapshot.host.uptime_secs = Some(90);
        snapshot.services = vec![ServiceStatus {
            subject: "immich".into(),
            active: false,
        }];
        snapshot
            .health
            .problems
            .push(outage("immich", Severity::Critical));
        assert_eq!(plan(&snapshot, &config()), [Page::Startup]);

        // Past the window the same outage alarms again.
        snapshot.host.uptime_secs = Some(1200);
        assert!(!startup_relevant(&snapshot, &config()));
        assert_eq!(plan(&snapshot, &config()), [Page::Alarm]);

        // Hardware is broken even during boot.
        snapshot.host.uptime_secs = Some(90);
        snapshot.disks.push(Disk::default());
        assert_eq!(plan(&snapshot, &config()), [Page::Alarm]);
    }

    #[test]
    fn startup_page_ends_when_every_watched_service_is_active() {
        let mut snapshot = Snapshot::default();
        snapshot.host.uptime_secs = Some(60);
        snapshot.services = vec![ServiceStatus {
            subject: "docker".into(),
            active: true,
        }];
        assert_eq!(
            plan(&snapshot, &config()),
            [Page::Home, Page::Temps, Page::Load, Page::Space]
        );
    }

    #[test]
    fn failed_disks_and_degraded_pool_each_get_a_line() {
        let snapshot = Snapshot {
            disks: vec![
                Disk {
                    label: "H1".into(),
                    present: true,
                    ..Disk::default()
                },
                Disk {
                    label: "H2".into(),
                    present: false,
                    ..Disk::default()
                },
                Disk {
                    label: "H3".into(),
                    present: true,
                    smart_failed: true,
                    ..Disk::default()
                },
            ],
            pools: vec![Pool {
                name: "storage01".into(),
                health: "DEGRADED".into(),
                bad_devices: vec![BadDevice {
                    name: "H2".into(),
                    state: "UNAVAIL".into(),
                }],
                ..Pool::default()
            }],
            ..Snapshot::default()
        };
        // The pool's own critical outage is not repeated as "не работает".
        let mut snapshot = snapshot;
        snapshot
            .health
            .problems
            .push(outage("storage01", Severity::Critical));
        let lines: Vec<String> = alarms(&snapshot, test_strings())
            .into_iter()
            .map(|item| format!("{}: {}", item.what, item.why))
            .collect();
        assert_eq!(
            lines,
            [
                "H2: пропал, UNAVAIL",
                "H3: SMART отказ",
                "storage01: DEGRADED",
            ]
        );
    }

    #[test]
    fn rotation_keeps_current_page_when_plan_changes() {
        let config = config();
        let start = Instant::now();
        let mut rotation = Rotation::new(start);
        let calm = Snapshot::default();
        assert_eq!(rotation.tick(&calm, &config, start).0, Page::Home);
        let home = dwell(Page::Home, &config);
        assert_eq!(rotation.tick(&calm, &config, start + home).0, Page::Temps);

        let mut busy = Snapshot::default();
        busy.ci.running.push(Pipeline::default());
        let later = start + home + Duration::from_secs(1);
        assert_eq!(rotation.tick(&busy, &config, later).0, Page::Temps);
    }
}
