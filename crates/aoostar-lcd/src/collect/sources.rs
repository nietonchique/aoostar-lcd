//! Slow metric sources as adapters.
//!
//! Every source owns its fetcher and a staged result: [`SlowSource::fetch`]
//! runs on the worker thread with no locks held (HTTP calls, child
//! processes), [`SlowSource::commit`] moves the staged outcome into the
//! shared [`Slots`] under a short lock, so `Collector::snapshot` never waits
//! for I/O. Adding a source means writing one adapter and listing it in
//! [`Sources::adapters`].

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use super::alerts::{self, Alert, FetchError, NO_TOKEN};
use super::docker::{Docker, DockerState};
use super::gpu;
use super::prometheus::{self, Queue};
use super::smart::{self, SmartInfo};
use super::systemd::{self, SystemdState};
use super::zfs::{self};
use crate::config::Config;
use crate::model::{DiskKind, Gpu, Pool};

fn describe(error: anyhow::Error) -> String {
    format!("{error:#}")
}

/// Last good value of a slow source plus the most recent failure.
struct Cached<T> {
    value: Option<(T, Instant)>,
    error: Option<String>,
}

impl<T> Default for Cached<T> {
    fn default() -> Self {
        Self {
            value: None,
            error: None,
        }
    }
}

impl<T> Cached<T> {
    fn record(&mut self, result: Result<T, String>) {
        match result {
            Ok(value) => {
                self.value = Some((value, Instant::now()));
                self.error = None;
            }
            Err(reason) => self.error = Some(reason),
        }
    }

    /// Failure that invalidates the previous value immediately.
    fn record_hard_failure(&mut self, reason: String) {
        self.value = None;
        self.error = Some(reason);
    }

    /// The value, unless it is older than three refresh intervals.
    fn fresh(&self, now: Instant, interval: Duration) -> Option<&T> {
        let (value, at) = self.value.as_ref()?;
        (now.saturating_duration_since(*at) <= interval * super::GRACE_INTERVALS).then_some(value)
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn has_value(&self) -> bool {
        self.value.is_some()
    }
}

/// Shared per-source results the snapshot merge reads.
#[derive(Default)]
pub struct Slots {
    docker: Cached<DockerState>,
    systemd: Cached<SystemdState>,
    gpu: Cached<Gpu>,
    alerts: Cached<Vec<Alert>>,
    queue: Cached<Queue>,
    pools: Cached<Vec<Pool>>,
    /// Keyed by the configured by-id path.
    smart: HashMap<String, Cached<SmartInfo>>,
}

impl Slots {
    pub fn docker(&self, now: Instant) -> Option<&DockerState> {
        self.docker.fresh(now, DockerSource::INTERVAL)
    }

    pub fn systemd(&self, now: Instant) -> Option<&SystemdState> {
        self.systemd.fresh(now, SystemdSource::INTERVAL)
    }

    pub fn gpu(&self, now: Instant) -> Option<&Gpu> {
        self.gpu.fresh(now, GpuSource::INTERVAL)
    }

    pub fn alerts(&self, now: Instant) -> Option<&Vec<Alert>> {
        self.alerts.fresh(now, AlertsSource::INTERVAL)
    }

    pub fn alerts_error(&self) -> Option<&str> {
        self.alerts.error()
    }

    pub fn alerts_ever_fetched(&self) -> bool {
        self.alerts.has_value()
    }

    pub fn queue(&self, now: Instant) -> Option<&Queue> {
        self.queue.fresh(now, QueueSource::INTERVAL)
    }

    pub fn pools(&self, now: Instant) -> Option<&Vec<Pool>> {
        self.pools.fresh(now, PoolsSource::INTERVAL)
    }

    pub fn smart(&self, key: &str, now: Instant) -> Option<&SmartInfo> {
        let cached = self.smart.get(key)?;
        cached.fresh(now, SmartSource::INTERVAL)
    }
}

/// One slow source: fetches on the worker thread, commits into [`Slots`].
pub(crate) trait SlowSource {
    /// Whether the configuration enables this source.
    fn applies(&self, _config: &Config) -> bool {
        true
    }

    /// Refresh cadence.
    fn interval(&self) -> Duration;

    /// Fetch and stage the outcome; runs with no locks held.
    fn fetch(&mut self, config: &Config);

    /// Move the staged outcome into the shared slots; short lock only.
    fn commit(&mut self, slots: &mut Slots);
}

// ------------------------------------------------------------------ docker

pub struct DockerSource {
    docker: Docker,
    staged: Option<Result<DockerState, String>>,
}

impl DockerSource {
    pub const INTERVAL: Duration = Duration::from_secs(5);

    pub fn new(config: &Config) -> Self {
        Self {
            docker: Docker::new(&config.services.docker_socket),
            staged: None,
        }
    }
}

impl SlowSource for DockerSource {
    fn interval(&self) -> Duration {
        Self::INTERVAL
    }

    fn fetch(&mut self, _config: &Config) {
        self.staged = Some(self.docker.refresh().map_err(describe));
    }

    fn commit(&mut self, slots: &mut Slots) {
        if let Some(result) = self.staged.take() {
            slots.docker.record(result);
        }
    }
}

// ----------------------------------------------------------------- systemd

pub struct SystemdSource {
    staged: Option<Result<SystemdState, String>>,
}

impl SystemdSource {
    pub const INTERVAL: Duration = Duration::from_secs(5);

    pub fn new() -> Self {
        Self { staged: None }
    }
}

impl Default for SystemdSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SlowSource for SystemdSource {
    fn interval(&self) -> Duration {
        Self::INTERVAL
    }

    fn fetch(&mut self, config: &Config) {
        self.staged = Some(systemd::read(&config.services).map_err(describe));
    }

    fn commit(&mut self, slots: &mut Slots) {
        if let Some(result) = self.staged.take() {
            slots.systemd.record(result);
        }
    }
}

// ---------------------------------------------------------------------- gpu

pub struct GpuSource {
    staged: Option<Result<Gpu, String>>,
}

impl GpuSource {
    pub const INTERVAL: Duration = Duration::from_secs(5);

    pub fn new() -> Self {
        Self { staged: None }
    }
}

impl Default for GpuSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SlowSource for GpuSource {
    fn interval(&self) -> Duration {
        Self::INTERVAL
    }

    fn fetch(&mut self, _config: &Config) {
        self.staged = Some(gpu::read().map_err(describe));
    }

    fn commit(&mut self, slots: &mut Slots) {
        if let Some(result) = self.staged.take() {
            slots.gpu.record(result);
        }
    }
}

// ------------------------------------------------------------------- alerts

pub struct AlertsSource {
    staged: Option<Result<Vec<Alert>, FetchError>>,
}

impl AlertsSource {
    pub const INTERVAL: Duration = Duration::from_secs(15);

    pub fn new() -> Self {
        Self { staged: None }
    }
}

impl Default for AlertsSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SlowSource for AlertsSource {
    fn applies(&self, config: &Config) -> bool {
        config.alerts.is_some()
    }

    fn interval(&self) -> Duration {
        Self::INTERVAL
    }

    fn fetch(&mut self, config: &Config) {
        let Some(alerts_config) = &config.alerts else {
            return;
        };
        self.staged = Some(alerts::fetch(alerts_config));
    }

    fn commit(&mut self, slots: &mut Slots) {
        let Some(result) = self.staged.take() else {
            return;
        };
        match result {
            Ok(list) => slots.alerts.record(Ok(list)),
            Err(FetchError::NoToken) => {
                slots.alerts.record_hard_failure(NO_TOKEN.to_owned());
            }
            Err(error @ FetchError::Failed(_)) => slots.alerts.record(Err(error.reason())),
        }
    }
}

// -------------------------------------------------------------------- pools

pub struct PoolsSource {
    staged: Option<Result<Vec<Pool>, String>>,
}

impl PoolsSource {
    pub const INTERVAL: Duration = Duration::from_secs(30);

    pub fn new() -> Self {
        Self { staged: None }
    }
}

impl Default for PoolsSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SlowSource for PoolsSource {
    fn applies(&self, config: &Config) -> bool {
        config.zfs.is_some()
    }

    fn interval(&self) -> Duration {
        Self::INTERVAL
    }

    fn fetch(&mut self, config: &Config) {
        let Some(zfs_config) = &config.zfs else {
            return;
        };
        let labels = disk_labels(config);
        let label = |path: &str| {
            block_device_of(Path::new(path)).and_then(|disk| labels.get(&disk).cloned())
        };
        self.staged = Some(
            zfs_config
                .pools
                .iter()
                .map(|pool| zfs::read_pool(pool, &label))
                .collect::<anyhow::Result<Vec<_>>>()
                .map_err(describe),
        );
    }

    fn commit(&mut self, slots: &mut Slots) {
        if let Some(result) = self.staged.take() {
            slots.pools.record(result);
        }
    }
}

/// Kernel block-device name (`sdb`) -> configured disk label (`H2`).
fn disk_labels(config: &Config) -> HashMap<String, String> {
    config
        .disks
        .iter()
        .filter_map(|disk| {
            let name = block_device_of(&disk.by_id)?;
            Some((name, disk.label.clone()))
        })
        .collect()
}

/// Whole-disk kernel name for a device or partition path
/// (`/dev/disk/by-partuuid/…` -> `sdb`).
fn block_device_of(path: &Path) -> Option<String> {
    let device = std::fs::canonicalize(path).ok()?;
    let name = device.file_name()?.to_string_lossy().into_owned();
    let sys = Path::new("/sys/class/block").join(&name);
    if sys.join("partition").exists() {
        let parent = std::fs::canonicalize(&sys).ok()?;
        return Some(parent.parent()?.file_name()?.to_string_lossy().into_owned());
    }
    Some(name)
}

// -------------------------------------------------------------------- queue

pub struct QueueSource {
    staged: Option<Result<Queue, String>>,
}

impl QueueSource {
    pub const INTERVAL: Duration = Duration::from_secs(30);

    pub fn new() -> Self {
        Self { staged: None }
    }
}

impl Default for QueueSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SlowSource for QueueSource {
    fn applies(&self, config: &Config) -> bool {
        config.ci.prometheus_url.is_some()
    }

    fn interval(&self) -> Duration {
        Self::INTERVAL
    }

    fn fetch(&mut self, config: &Config) {
        let Some(url) = &config.ci.prometheus_url else {
            return;
        };
        self.staged = Some(prometheus::fetch(url).map_err(describe));
    }

    fn commit(&mut self, slots: &mut Slots) {
        if let Some(result) = self.staged.take() {
            slots.queue.record(result);
        }
    }
}

// -------------------------------------------------------------------- smart

pub struct SmartSource {
    staged: Vec<(String, Result<SmartInfo, String>)>,
}

impl SmartSource {
    pub const INTERVAL: Duration = Duration::from_secs(60);

    pub fn new() -> Self {
        Self { staged: Vec::new() }
    }
}

impl Default for SmartSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SlowSource for SmartSource {
    fn applies(&self, config: &Config) -> bool {
        config.disks.iter().any(|disk| disk.kind == DiskKind::Hdd)
    }

    fn interval(&self) -> Duration {
        Self::INTERVAL
    }

    fn fetch(&mut self, config: &Config) {
        self.staged = config
            .disks
            .iter()
            .filter(|disk| disk.kind == DiskKind::Hdd)
            .map(|disk| {
                (
                    disk.by_id.to_string_lossy().into_owned(),
                    smart::read(&disk.by_id).map_err(describe),
                )
            })
            .collect();
    }

    fn commit(&mut self, slots: &mut Slots) {
        for (key, result) in self.staged.drain(..) {
            slots.smart.entry(key).or_default().record(result);
        }
    }
}

// ----------------------------------------------------------------- registry

/// All slow sources, in priority order: cheap local sources first so slow
/// network calls never delay them.
pub struct Sources {
    docker: DockerSource,
    systemd: SystemdSource,
    gpu: GpuSource,
    alerts: AlertsSource,
    pools: PoolsSource,
    queue: QueueSource,
    smart: SmartSource,
}

impl Sources {
    pub fn new(config: &Config) -> Self {
        Self {
            docker: DockerSource::new(config),
            systemd: SystemdSource::new(),
            gpu: GpuSource::new(),
            alerts: AlertsSource::new(),
            pools: PoolsSource::new(),
            queue: QueueSource::new(),
            smart: SmartSource::new(),
        }
    }

    /// The adapters in refresh-priority order.
    pub fn adapters(&mut self) -> [&mut dyn SlowSource; 7] {
        [
            &mut self.docker,
            &mut self.systemd,
            &mut self.gpu,
            &mut self.alerts,
            &mut self.pools,
            &mut self.queue,
            &mut self.smart,
        ]
    }

    /// Which configuration sections enable which adapters (docker, systemd,
    /// gpu, alerts, pools, queue, smart).
    #[cfg(test)]
    pub fn applies(&mut self, config: &Config) -> Vec<bool> {
        self.adapters()
            .iter()
            .map(|adapter| adapter.applies(config))
            .collect()
    }

    /// One synchronous pass over every enabled source.
    pub fn refresh_all(&mut self, config: &Config, slots: &mut Slots) {
        for adapter in self.adapters() {
            if !adapter.applies(config) {
                continue;
            }
            adapter.fetch(config);
            adapter.commit(slots);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    #[test]
    fn intervals_match_the_documented_ones() {
        assert_eq!(DockerSource::INTERVAL, Duration::from_secs(5));
        assert_eq!(SystemdSource::INTERVAL, Duration::from_secs(5));
        assert_eq!(GpuSource::INTERVAL, Duration::from_secs(5));
        assert_eq!(AlertsSource::INTERVAL, Duration::from_secs(15));
        assert_eq!(PoolsSource::INTERVAL, Duration::from_secs(30));
        assert_eq!(QueueSource::INTERVAL, Duration::from_secs(30));
        assert_eq!(SmartSource::INTERVAL, Duration::from_secs(60));
    }

    #[test]
    fn applies_follows_the_configuration() {
        let example = config(include_str!("../../../../config.example.toml"));
        let mut sources = Sources::new(&example);
        assert_eq!(
            sources.applies(&example),
            // docker, systemd, gpu are always on; alerts, pools, queue and
            // smart need their config sections or an HDD row.
            vec![true, true, true, false, false, false, false]
        );
    }

    #[test]
    fn configured_sources_are_enabled() {
        let full = config(
            "[display]\ndevice = \"/dev/serial/by-id/usb-x\"\n\
             [alerts]\ngrafana_url = \"http://grafana\"\n\
             token_file = \"/etc/aoostar-lcd/grafana.token\"\noutage_families = []\n\
             [[disk]]\nlabel = \"H1\"\nkind = \"hdd\"\nby_id = \"/dev/disk/by-id/ata-x\"\n\
             [zfs]\npools = [\"tank\"]\n[ci]\nprometheus_url = \"http://prom\"\n",
        );
        let mut sources = Sources::new(&full);
        assert_eq!(sources.applies(&full), vec![true; 7]);
    }

    #[test]
    fn cached_values_age_out_after_three_intervals() {
        let mut cached: Cached<u32> = Cached::default();
        let now = Instant::now();
        cached.record(Ok(7));
        assert_eq!(cached.fresh(now, Duration::from_secs(10)), Some(&7));
        assert_eq!(
            cached.fresh(now + Duration::from_secs(31), Duration::from_secs(10)),
            None
        );
        cached.record_hard_failure("gone".to_owned());
        assert_eq!(cached.fresh(now, Duration::from_secs(10)), None);
        assert_eq!(cached.error(), Some("gone"));
        assert!(!cached.has_value());
    }
}
