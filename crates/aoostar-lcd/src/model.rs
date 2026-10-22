//! Snapshot of everything the display can show.
//!
//! Collectors fill it, pages only read it. It is serializable so a live
//! snapshot captured on the server can be rendered offline (`aoostar-lcd preview`).
//! Every value that may be unavailable is an `Option`; renderers show `--`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// Unix seconds when the snapshot was assembled.
    pub taken_at: i64,
    pub host: Host,
    /// Configured physical disks in configuration order.
    pub disks: Vec<Disk>,
    /// Configured filesystems in configuration order.
    pub volumes: Vec<Volume>,
    /// Configured ZFS pools in configuration order.
    pub pools: Vec<Pool>,
    /// ZFS datasets with logical I/O in the last interval, busiest first.
    pub datasets: Vec<DatasetIo>,
    /// Busiest workloads by CPU, highest first.
    pub top_cpu: Vec<WorkloadCpu>,
    /// Largest workloads by resident memory, highest first.
    pub top_memory: Vec<WorkloadMemory>,
    pub ci: Ci,
    /// Watched services (stacks and units) and their systemd state; empty
    /// while the source has not answered yet.
    pub services: Vec<ServiceStatus>,
    pub health: Health,
}

/// One watched service and whether systemd currently reports it active.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceStatus {
    /// Stack name for template instances, otherwise the unit name without
    /// its type suffix.
    pub subject: String,
    pub active: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Host {
    /// Whole-host CPU utilisation, 0..=100.
    pub cpu_percent: Option<f32>,
    pub load1: Option<f32>,
    pub cpu_temp_c: Option<f32>,
    /// Hottest DIMM sensor.
    pub ram_temp_c: Option<f32>,
    pub mem_total: u64,
    pub mem_available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub gpu: Option<Gpu>,
    /// Spinning chassis fans only (0 rpm headers are omitted).
    pub fans: Vec<Fan>,
    /// Seconds since boot, from `/proc/uptime`.
    pub uptime_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Gpu {
    pub util_percent: Option<f32>,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub mem_used_mib: Option<u64>,
    pub mem_total_mib: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Fan {
    pub label: String,
    pub rpm: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiskKind {
    #[default]
    Nvme,
    Hdd,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Disk {
    pub label: String,
    pub kind: DiskKind,
    /// False when the by-id path does not resolve to a block device.
    pub present: bool,
    pub temp_c: Option<f32>,
    pub read_bps: u64,
    pub write_bps: u64,
    pub read_iops: u64,
    pub write_iops: u64,
    /// Share of wall time with I/O in flight (diskstats io_ticks), 0..=100.
    pub busy_percent: f32,
    /// SMART self-test currently executing.
    pub self_test: Option<SelfTest>,
    /// SMART overall health assessment reported FAILED.
    pub smart_failed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SelfTest {
    /// Remaining work, 0..=100, when the drive reports it.
    pub remaining_percent: Option<u8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Volume {
    pub label: String,
    /// False when the configured path is not a mounted filesystem.
    pub mounted: bool,
    pub used: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Pool {
    pub name: String,
    /// `ONLINE`, `DEGRADED`, ... as reported by `zpool status`.
    pub health: String,
    pub used: u64,
    pub total: u64,
    /// Sum of read/write/checksum errors over all leaf vdevs.
    pub errors: u64,
    /// Leaf devices that are not ONLINE (disk label when known).
    pub bad_devices: Vec<BadDevice>,
    pub scan: Option<PoolScan>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BadDevice {
    /// Configured disk label (`H3`) or the raw vdev name.
    pub name: String,
    /// `DEGRADED`, `FAULTED`, `UNAVAIL`, `REMOVED`, `OFFLINE`, ...
    pub state: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanKind {
    #[default]
    Scrub,
    Resilver,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolScan {
    pub kind: ScanKind,
    pub paused: bool,
    /// 0..=100.
    pub percent: f32,
    /// Estimated seconds to completion while actively scanning.
    pub eta_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DatasetIo {
    /// Dataset name without the pool prefix, e.g. `backups/urbackup`.
    pub name: String,
    pub read_bps: u64,
    pub write_bps: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkloadCpu {
    pub name: String,
    /// Share of the whole host (all logical CPUs), 0..=100.
    pub percent: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkloadMemory {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ci {
    /// False when the Docker API could not be read.
    pub available: bool,
    /// Running pipelines, oldest first.
    pub running: Vec<Pipeline>,
    /// Tasks waiting for an agent; `None` when Prometheus is unavailable.
    pub queued: Option<u32>,
    pub oldest_queued_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Pipeline {
    /// `owner/name`.
    pub repo: String,
    pub number: u64,
    pub branch: String,
    /// Currently running step names.
    pub steps: Vec<String>,
    /// Unix seconds when the pipeline started.
    pub started_at: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Intentionally stopped (maintenance flag present). Ordered lowest.
    Maintenance,
    #[default]
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProblemClass {
    /// Something is not reachable/running: drives the red status bar.
    Outage,
    /// Any other firing alert (capacity, backup freshness, ...).
    #[default]
    Alert,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Problem {
    pub class: ProblemClass,
    pub severity: Severity,
    /// Service / stack / unit the problem belongs to; the row title.
    pub subject: String,
    /// Short human description, e.g. `HTTPS probe failed` or `3 alerts: backup stale`.
    pub detail: String,
    /// Unix seconds of the earliest start among merged items.
    pub since: Option<i64>,
    /// Number of merged underlying alerts/checks (>= 1).
    pub count: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "state", content = "reason")]
pub enum SourceState {
    #[default]
    Ok,
    Unavailable(String),
    Disabled,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Health {
    /// Sorted: outages before alerts, then severity descending, then oldest first.
    pub problems: Vec<Problem>,
    /// State of the Grafana alert source.
    pub alerts: SourceState,
    pub containers_running: u32,
    pub containers_total: u32,
}

impl Health {
    pub fn outages(&self) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(|problem| problem.class == ProblemClass::Outage)
    }

    pub fn alerts(&self) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(|problem| problem.class == ProblemClass::Alert)
    }

    /// Outages that are not intentional maintenance.
    pub fn active_outages(&self) -> impl Iterator<Item = &Problem> {
        self.outages()
            .filter(|problem| problem.severity != Severity::Maintenance)
    }
}

impl Snapshot {
    /// Highest HDD utilisation, used to decide whether the HDD page is relevant.
    pub fn hdd_busy_max(&self) -> f32 {
        self.disks
            .iter()
            .filter(|disk| disk.kind == DiskKind::Hdd)
            .map(|disk| disk.busy_percent)
            .fold(0.0, f32::max)
    }

    pub fn hdd_self_test_running(&self) -> bool {
        self.disks
            .iter()
            .any(|disk| disk.kind == DiskKind::Hdd && disk.self_test.is_some())
    }
}
