//! systemd: compose stack instances, required units, failed units and
//! maintenance flags. Read-only `systemctl` queries.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Duration;

use anyhow::{Context, bail};

use crate::config::ServicesConfig;

use super::proc;

const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(4);
const SHOW_PROPERTIES: &str =
    "Id,LoadState,ActiveState,StateChangeTimestampMonotonic,Type,RemainAfterExit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitState {
    /// Name used for problems: stack name for template instances, otherwise
    /// the unit name without its type suffix.
    pub subject: String,
    /// `loaded`, `not-found`, ...
    pub load: String,
    /// `active`, `inactive`, `failed`, `activating`, ...
    pub active: String,
    /// Unix seconds of the last state change, when known.
    pub since: Option<i64>,
    /// A run-to-completion job (`Type=oneshot` without `RemainAfterExit`),
    /// e.g. a timer-driven backup. Its failure is not a service outage.
    pub job: bool,
}

impl UnitState {
    pub fn is_active(&self) -> bool {
        self.active == "active"
    }
}

#[derive(Debug, Clone, Default)]
pub struct SystemdState {
    /// Loaded instances of the configured template units.
    pub stacks: Vec<UnitState>,
    /// Configured individual units.
    pub units: Vec<UnitState>,
    /// Units in the `failed` state.
    pub failed: Vec<UnitState>,
    /// Names from the maintenance directory (`ALL` included).
    pub maintenance: HashSet<String>,
}

impl SystemdState {
    pub fn in_maintenance(&self, subject: &str) -> bool {
        self.maintenance.contains("ALL") || self.maintenance.contains(subject)
    }
}

/// One row of `systemctl list-units --plain --no-legend`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub unit: String,
    pub load: String,
    pub active: String,
}

pub fn parse_list_units(text: &str) -> Vec<Listed> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line
                .split_whitespace()
                // Defensive: a status bullet may precede the unit name.
                .skip_while(|field| matches!(*field, "●" | "*" | "○" | "×"));
            let unit = fields.next()?.to_owned();
            let load = fields.next()?.to_owned();
            let active = fields.next()?.to_owned();
            // The sub-state column must exist for the row to be well-formed.
            fields.next()?;
            Some(Listed { unit, load, active })
        })
        .collect()
}

/// One block of `systemctl show` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub unit: String,
    pub load: String,
    pub active: String,
    /// Microseconds on CLOCK_MONOTONIC; 0 = never changed.
    pub changed_monotonic_us: u64,
    /// `Type=oneshot` and `RemainAfterExit=no`.
    pub job: bool,
}

pub fn parse_show(text: &str) -> Vec<Shown> {
    let mut shown = Vec::new();
    for block in text.split("\n\n") {
        let mut properties: HashMap<&str, &str> = HashMap::new();
        for line in block.lines() {
            if let Some((key, value)) = line.split_once('=') {
                properties.insert(key.trim(), value.trim());
            }
        }
        let Some(unit) = properties.get("Id").filter(|id| !id.is_empty()) else {
            continue;
        };
        shown.push(Shown {
            unit: (*unit).to_owned(),
            load: properties.get("LoadState").unwrap_or(&"").to_string(),
            active: properties.get("ActiveState").unwrap_or(&"").to_string(),
            changed_monotonic_us: properties
                .get("StateChangeTimestampMonotonic")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            job: properties.get("Type") == Some(&"oneshot")
                && properties.get("RemainAfterExit") != Some(&"yes"),
        });
    }
    shown
}

/// Subject for a unit: the stack name for instances of a configured template
/// (`homelab-compose@shop.service` -> `shop`), else the unit name without its
/// type suffix (`docker.service` -> `docker`).
pub fn subject_of(unit: &str, templates: &[String]) -> String {
    let stem = unit.rsplit_once('.').map_or(unit, |(stem, _)| stem);
    for template in templates {
        if let Some(instance) = stem.strip_prefix(template.as_str())
            && !instance.is_empty()
        {
            return instance.to_owned();
        }
    }
    stem.to_owned()
}

/// Wall-clock time of a monotonic timestamp given the boot time.
pub fn since_from_monotonic(boot_unix: i64, monotonic_us: u64) -> Option<i64> {
    (monotonic_us > 0).then(|| boot_unix + (monotonic_us / 1_000_000) as i64)
}

/// `/proc/uptime` first field in seconds.
pub fn parse_uptime(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse().ok()
}

fn systemctl(args: &[&str]) -> anyhow::Result<String> {
    let output = proc::run("systemctl", args, SYSTEMCTL_TIMEOUT).context("systemctl")?;
    if !output.success {
        bail!("systemctl {} failed", args.first().unwrap_or(&""));
    }
    Ok(output.stdout)
}

pub fn read(config: &ServicesConfig) -> anyhow::Result<SystemdState> {
    let now = super::unix_now();
    let boot_unix = fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| parse_uptime(&text))
        .map_or(0, |uptime| now - uptime as i64);

    let mut list_args = vec![
        "list-units",
        "--all",
        "--plain",
        "--no-legend",
        "--no-pager",
    ];
    let patterns: Vec<String> = config
        .templates
        .iter()
        .map(|template| format!("{template}*"))
        .collect();
    let mut stack_units = Vec::new();
    if !patterns.is_empty() {
        list_args.extend(patterns.iter().map(String::as_str));
        stack_units = parse_list_units(&systemctl(&list_args)?);
        stack_units.retain(|listed| {
            listed.load == "loaded"
                && config
                    .templates
                    .iter()
                    .any(|template| listed.unit.starts_with(template.as_str()))
        });
    }
    let failed_units = parse_list_units(&systemctl(&[
        "list-units",
        "--state=failed",
        "--plain",
        "--no-legend",
        "--no-pager",
    ])?);

    // One `show` call for everything whose details are needed: configured
    // units (always) and any stack/failed unit that is not active.
    let mut wanted: Vec<&str> = Vec::new();
    wanted.extend(config.units.iter().map(String::as_str));
    wanted.extend(
        stack_units
            .iter()
            .filter(|listed| listed.active != "active")
            .map(|listed| listed.unit.as_str()),
    );
    wanted.extend(failed_units.iter().map(|listed| listed.unit.as_str()));
    let mut seen = HashSet::new();
    wanted.retain(|unit| seen.insert(*unit));

    let mut details: HashMap<String, Shown> = HashMap::new();
    if !wanted.is_empty() {
        let mut show_args = vec!["show", "--no-pager", "-p", SHOW_PROPERTIES];
        show_args.extend(wanted.iter().copied());
        for shown in parse_show(&systemctl(&show_args)?) {
            details.insert(shown.unit.clone(), shown);
        }
    }

    let templates = &config.templates;
    let from_listed = |listed: &Listed| -> UnitState {
        let shown = details.get(&listed.unit);
        UnitState {
            subject: subject_of(&listed.unit, templates),
            load: listed.load.clone(),
            active: shown.map_or_else(|| listed.active.clone(), |s| s.active.clone()),
            since: shown.and_then(|s| since_from_monotonic(boot_unix, s.changed_monotonic_us)),
            job: shown.is_some_and(|s| s.job),
        }
    };

    let stacks = stack_units.iter().map(from_listed).collect();
    let failed = failed_units.iter().map(from_listed).collect();
    let units = config
        .units
        .iter()
        .map(|unit| {
            let shown = details.get(unit);
            UnitState {
                subject: subject_of(unit, templates),
                load: shown.map_or_else(|| "not-found".to_owned(), |s| s.load.clone()),
                active: shown.map_or_else(|| "inactive".to_owned(), |s| s.active.clone()),
                since: shown.and_then(|s| since_from_monotonic(boot_unix, s.changed_monotonic_us)),
                job: shown.is_some_and(|s| s.job),
            }
        })
        .collect();

    Ok(SystemdState {
        stacks,
        units,
        failed,
        maintenance: read_maintenance(config),
    })
}

fn read_maintenance(config: &ServicesConfig) -> HashSet<String> {
    fs::read_dir(&config.maintenance_dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// System power state the stop jobs in `systemctl list-jobs` belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Poweroff,
    Reboot,
}

/// Target of the shutdown in progress: the `start` job of `reboot.target`,
/// `poweroff.target` or a halt-like target. `None` while the system is not
/// shutting down — a manual unit stop or a deploy restart.
pub fn parse_jobs(text: &str) -> Option<Target> {
    text.lines().find_map(|line| {
        // `JOB UNIT TYPE STATE`, e.g. `412 reboot.target start running`.
        let mut fields = line.split_whitespace();
        let _job = fields.next()?;
        match (fields.next()?, fields.next()?) {
            ("reboot.target", "start") => Some(Target::Reboot),
            ("poweroff.target", "start") | ("halt.target", "start") | ("kexec.target", "start") => {
                Some(Target::Poweroff)
            }
            _ => None,
        }
    })
}

/// Which power transition is running, if any.
pub fn shutdown_target() -> Option<Target> {
    let jobs = systemctl(&["list-jobs", "--plain", "--no-legend", "--no-pager"]).ok()?;
    parse_jobs(&jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "\
homelab-compose@adengard.service    loaded active   exited Homelab Compose stack adengard
homelab-compose@dockge.service      loaded inactive dead   Homelab Compose stack dockge
homelab-compose@ghost.service       not-found inactive dead homelab-compose@ghost.service
";

    #[test]
    fn list_units_rows() {
        let rows = parse_list_units(LIST);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].unit, "homelab-compose@adengard.service");
        assert_eq!(rows[0].load, "loaded");
        assert_eq!(rows[0].active, "active");
        assert_eq!(rows[1].active, "inactive");
        assert_eq!(rows[2].load, "not-found");
        assert!(parse_list_units("").is_empty());
        assert!(parse_list_units("short line\n").is_empty());
    }

    #[test]
    fn list_units_tolerates_status_bullet() {
        let rows = parse_list_units("● smb.service loaded failed failed Samba\n");
        assert_eq!(rows[0].unit, "smb.service");
        assert_eq!(rows[0].active, "failed");
    }

    #[test]
    fn list_jobs_names_the_power_target() {
        let jobs = "412 docker.service stop done\n413 reboot.target start running\n";
        assert_eq!(parse_jobs(jobs), Some(Target::Reboot));
        let jobs =
            "1 homelab-compose@gitea.service stop running\n2 poweroff.target start waiting\n";
        assert_eq!(parse_jobs(jobs), Some(Target::Poweroff));
        assert_eq!(
            parse_jobs("1 poweroff.target start waiting\n2 reboot.target start running\n"),
            Some(Target::Poweroff)
        );
        assert_eq!(parse_jobs("412 docker.service stop running\n"), None);
        assert_eq!(parse_jobs(""), None);
    }

    #[test]
    fn show_blocks() {
        let text = "\
Id=docker.service
LoadState=loaded
ActiveState=active
StateChangeTimestampMonotonic=142685242939
Type=notify
RemainAfterExit=no

Id=nosuch.service
LoadState=not-found
ActiveState=inactive
StateChangeTimestampMonotonic=0

Id=granite-immich-library-backup.service
LoadState=loaded
ActiveState=failed
StateChangeTimestampMonotonic=1
Type=oneshot
RemainAfterExit=no

Id=homelab-compose@shop.service
LoadState=loaded
ActiveState=failed
StateChangeTimestampMonotonic=1
Type=oneshot
RemainAfterExit=yes
";
        let shown = parse_show(text);
        assert_eq!(shown.len(), 4);
        assert_eq!(shown[0].unit, "docker.service");
        assert_eq!(shown[0].active, "active");
        assert_eq!(shown[0].changed_monotonic_us, 142_685_242_939);
        assert_eq!(shown[1].load, "not-found");
        assert_eq!(shown[1].changed_monotonic_us, 0);
        // Only a run-to-completion oneshot is a job; a RemainAfterExit stack is a service.
        assert_eq!(
            shown.iter().map(|s| s.job).collect::<Vec<_>>(),
            [false, false, true, false]
        );
    }

    #[test]
    fn subjects() {
        let templates = vec!["homelab-compose@".to_owned()];
        assert_eq!(
            subject_of("homelab-compose@shop.service", &templates),
            "shop"
        );
        assert_eq!(
            subject_of("homelab-compose@my.stack.service", &templates),
            "my.stack"
        );
        assert_eq!(subject_of("docker.service", &templates), "docker");
        assert_eq!(subject_of("user@1000.service", &templates), "user@1000");
        assert_eq!(subject_of("plain", &templates), "plain");
    }

    #[test]
    fn monotonic_to_wall_clock() {
        assert_eq!(since_from_monotonic(1_000_000, 5_500_000), Some(1_000_005));
        assert_eq!(since_from_monotonic(1_000_000, 0), None);
        assert_eq!(parse_uptime("219233.75 2192689.37\n"), Some(219_233.75));
    }

    #[test]
    fn maintenance_flags() {
        let mut state = SystemdState::default();
        state.maintenance.insert("shop".into());
        assert!(state.in_maintenance("shop"));
        assert!(!state.in_maintenance("other"));
        state.maintenance.insert("ALL".into());
        assert!(state.in_maintenance("other"));
    }
}
