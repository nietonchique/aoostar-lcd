//! Runtime configuration (`/etc/aoostar-lcd.toml`).

use anyhow::{Context, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::model::DiskKind;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub display: DisplayConfig,
    #[serde(default)]
    pub thresholds: Thresholds,
    /// Physical disks; by-id is the only accepted identity.
    #[serde(default, rename = "disk")]
    pub disks: Vec<DiskConfig>,
    /// Filesystems whose free space is shown.
    #[serde(default, rename = "volume")]
    pub volumes: Vec<VolumeConfig>,
    #[serde(default)]
    pub zfs: Option<ZfsConfig>,
    #[serde(default)]
    pub services: ServicesConfig,
    #[serde(default)]
    pub alerts: Option<AlertsConfig>,
    #[serde(default)]
    pub ci: CiConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    En,
    Ru,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayConfig {
    /// Serial device of the front LCD.
    pub device: PathBuf,
    /// Panel UI language.
    #[serde(default)]
    pub language: Lang,
    /// Seconds each page stays on screen.
    #[serde(default = "default_page_seconds")]
    pub page_seconds: u64,
    /// Heartbeat file touched after every frame successfully sent to the LCD.
    #[serde(default = "default_heartbeat")]
    pub heartbeat: PathBuf,
    /// Offset of the wall clock shown in the status bar (Granite runs in UTC).
    #[serde(default)]
    pub clock_utc_offset_minutes: i32,
}

fn default_page_seconds() -> u64 {
    10
}

fn default_heartbeat() -> PathBuf {
    PathBuf::from("/run/aoostar-lcd/heartbeat")
}

/// `[warn, critical]` boundaries. Values at or above a boundary take its colour.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Level(pub f32, pub f32);

impl Level {
    pub fn classify(self, value: f32) -> Tone {
        if value >= self.1 {
            Tone::Critical
        } else if value >= self.0 {
            Tone::Warning
        } else {
            Tone::Normal
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Thresholds {
    pub cpu_temp_c: Level,
    pub gpu_temp_c: Level,
    pub nvme_temp_c: Level,
    pub hdd_temp_c: Level,
    pub ram_temp_c: Level,
    /// Used-memory percentage.
    pub memory_used_percent: Level,
    /// Used-space percentage of volumes and the pool.
    pub space_used_percent: Level,
    /// HDD busy percentage: above the first value the HDD page joins the rotation.
    pub hdd_busy_percent: Level,
    /// Pipeline age in minutes.
    pub ci_minutes: Level,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            cpu_temp_c: Level(88.0, 95.0),
            gpu_temp_c: Level(80.0, 88.0),
            nvme_temp_c: Level(65.0, 75.0),
            hdd_temp_c: Level(45.0, 50.0),
            ram_temp_c: Level(70.0, 80.0),
            memory_used_percent: Level(85.0, 95.0),
            space_used_percent: Level(85.0, 95.0),
            hdd_busy_percent: Level(10.0, 80.0),
            ci_minutes: Level(15.0, 45.0),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskConfig {
    /// Short label shown on screen, e.g. `H1`.
    pub label: String,
    pub kind: DiskKind,
    pub by_id: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeConfig {
    pub label: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZfsConfig {
    /// Pools to watch; the first one is primary (its dataset names are shown
    /// without the pool prefix).
    pub pools: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServicesConfig {
    /// Every loaded instance of these template units must be active.
    pub templates: Vec<String>,
    /// Individual units that must be active.
    pub units: Vec<String>,
    /// `<dir>/<name>` or `<dir>/ALL` marks a stopped unit as intentional.
    pub maintenance_dir: PathBuf,
    pub docker_socket: PathBuf,
    /// Boot window: the startup page replaces the rotation until every
    /// watched service is active or this many seconds have passed since boot.
    #[serde(default = "default_startup_window_secs")]
    pub startup_window_secs: u64,
}

fn default_startup_window_secs() -> u64 {
    600
}

impl Default for ServicesConfig {
    fn default() -> Self {
        Self {
            templates: Vec::new(),
            units: Vec::new(),
            maintenance_dir: PathBuf::from("/run/homelab/maintenance"),
            docker_socket: PathBuf::from("/run/docker.sock"),
            startup_window_secs: default_startup_window_secs(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertsConfig {
    /// Grafana base URL reachable from the host, e.g. `http://172.16.7.2:3000`.
    pub grafana_url: String,
    /// Host header Grafana expects (it redirects other hosts to its root URL).
    pub grafana_host: Option<String>,
    /// Root-only file with a Viewer service-account token.
    pub token_file: PathBuf,
    /// `alert_family` labels that mean "something is unavailable".
    pub outage_families: Vec<String>,
    /// Alerts whose `node` label differs are prefixed with their node name.
    pub local_node: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CiConfig {
    /// Prometheus base URL for Woodpecker queue metrics; omitted = queue unknown.
    pub prometheus_url: Option<String>,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let config: Self = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.display.page_seconds == 0 {
            bail!("display.page_seconds must be positive");
        }
        if self.zfs.as_ref().is_some_and(|zfs| zfs.pools.is_empty()) {
            bail!("zfs.pools must name at least one pool");
        }
        let mut labels = HashSet::new();
        for disk in &self.disks {
            if !disk.by_id.starts_with("/dev/disk/by-id/") {
                bail!("disk {} must use a /dev/disk/by-id path", disk.label);
            }
            if disk.label.is_empty() || disk.label.chars().count() > 4 {
                bail!("disk label {:?} must be 1-4 characters", disk.label);
            }
            if !labels.insert(disk.label.as_str()) {
                bail!("duplicate disk label {}", disk.label);
            }
        }
        for volume in &self.volumes {
            if volume.label.is_empty() || volume.label.chars().count() > 5 {
                bail!("volume label {:?} must be 1-5 characters", volume.label);
            }
        }
        for (name, level) in [
            ("cpu_temp_c", self.thresholds.cpu_temp_c),
            ("gpu_temp_c", self.thresholds.gpu_temp_c),
            ("nvme_temp_c", self.thresholds.nvme_temp_c),
            ("hdd_temp_c", self.thresholds.hdd_temp_c),
            ("ram_temp_c", self.thresholds.ram_temp_c),
            ("memory_used_percent", self.thresholds.memory_used_percent),
            ("space_used_percent", self.thresholds.space_used_percent),
            ("hdd_busy_percent", self.thresholds.hdd_busy_percent),
            ("ci_minutes", self.thresholds.ci_minutes),
        ] {
            if level.0 > level.1 {
                bail!("thresholds.{name}: warning must not exceed critical");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_is_valid() {
        let text = include_str!("../../../config.example.toml");
        let config = Config::parse(text).unwrap();
        assert_eq!(config.display.language, Lang::En);
        assert!(config.services.units.contains(&"docker.service".to_owned()));
    }

    #[test]
    fn rejects_kernel_names_instead_of_by_id() {
        let error = Config::parse(
            "[display]\ndevice = \"/dev/ttyACM0\"\n[[disk]]\nlabel = \"H1\"\nkind = \"hdd\"\nby_id = \"/dev/sda\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("by-id"));
    }

    #[test]
    fn level_boundaries_are_inclusive() {
        let level = Level(80.0, 90.0);
        assert_eq!(level.classify(79.9), Tone::Normal);
        assert_eq!(level.classify(80.0), Tone::Warning);
        assert_eq!(level.classify(90.0), Tone::Critical);
    }
}
