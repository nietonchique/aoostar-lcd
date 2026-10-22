//! Read-only collectors.
//!
//! * Fast sources (`/proc`, hwmon, cgroups, statvfs) are sampled on the
//!   caller's thread inside [`Collector::snapshot`].
//! * Slow sources (Docker, systemd, nvidia-smi, Grafana, Prometheus, zpool,
//!   smartctl) are refreshed by one background thread, each on its own
//!   interval, into shared state that `snapshot` only reads under a short lock.
//!   A failing source keeps its last good value for three intervals and then
//!   degrades to unavailable.

mod alerts;
mod cgroups;
mod disks;
mod fast;
mod gpu;
mod health;
mod host;
mod proc;
mod prometheus;
mod smart;
mod volumes;
mod zfs;

// Crate-visible: the shutdown page reuses these collectors directly.
pub(crate) mod docker;
pub(crate) mod systemd;

use std::collections::HashMap;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::model::{Ci, DiskKind, Gpu, Host, Pool, ServiceStatus, Snapshot};

use alerts::{Alert, FetchError, NO_TOKEN};
use docker::{Docker, DockerState};
use fast::{Fast, Sample};
use health::{AlertsInput, Inputs};
use prometheus::Queue;
use smart::SmartInfo;
use systemd::SystemdState;

const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
/// A failed source keeps its last good value this many intervals.
const GRACE_INTERVALS: u32 = 3;
const WORKER_POLL: Duration = Duration::from_millis(200);

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .redirects(0)
        .build()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

// ---------------------------------------------------------------------------
// Slow-source state
// ---------------------------------------------------------------------------

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
        (now.saturating_duration_since(*at) <= interval * GRACE_INTERVALS).then_some(value)
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn has_value(&self) -> bool {
        self.value.is_some()
    }
}

#[derive(Default)]
struct SlowState {
    docker: Cached<DockerState>,
    systemd: Cached<SystemdState>,
    gpu: Cached<Gpu>,
    alerts: Cached<Vec<Alert>>,
    queue: Cached<Queue>,
    pools: Cached<Vec<Pool>>,
    /// Keyed by the configured by-id path.
    smart: HashMap<String, Cached<SmartInfo>>,
}

fn lock(state: &Mutex<SlowState>) -> MutexGuard<'_, SlowState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Docker,
    Systemd,
    Gpu,
    Alerts,
    Queue,
    Pool,
    Smart,
}

/// Priority order: cheap local sources first so slow network calls never delay them.
const JOBS: [Job; 7] = [
    Job::Docker,
    Job::Systemd,
    Job::Gpu,
    Job::Alerts,
    Job::Pool,
    Job::Queue,
    Job::Smart,
];

impl Job {
    fn interval(self) -> Duration {
        Duration::from_secs(match self {
            Self::Docker | Self::Systemd | Self::Gpu => 5,
            Self::Alerts => 15,
            Self::Queue | Self::Pool => 30,
            Self::Smart => 60,
        })
    }
}

struct Worker {
    config: Config,
    state: Arc<Mutex<SlowState>>,
    docker: Docker,
}

impl Worker {
    fn new(config: &Config, state: Arc<Mutex<SlowState>>) -> Self {
        Self {
            docker: Docker::new(&config.services.docker_socket),
            config: config.clone(),
            state,
        }
    }

    /// Whether the configuration enables this source.
    fn applies(&self, job: Job) -> bool {
        match job {
            Job::Docker | Job::Systemd | Job::Gpu => true,
            Job::Alerts => self.config.alerts.is_some(),
            Job::Queue => self.config.ci.prometheus_url.is_some(),
            Job::Pool => self.config.zfs.is_some(),
            Job::Smart => self.config.disks.iter().any(|d| d.kind == DiskKind::Hdd),
        }
    }

    /// Fetches without holding the lock, then stores the outcome.
    fn run(&mut self, job: Job) {
        fn describe(error: anyhow::Error) -> String {
            format!("{error:#}")
        }
        match job {
            Job::Docker => {
                let result = self.docker.refresh().map_err(describe);
                lock(&self.state).docker.record(result);
            }
            Job::Systemd => {
                let result = systemd::read(&self.config.services).map_err(describe);
                lock(&self.state).systemd.record(result);
            }
            Job::Gpu => {
                let result = gpu::read().map_err(describe);
                lock(&self.state).gpu.record(result);
            }
            Job::Alerts => {
                let Some(alerts_config) = &self.config.alerts else {
                    return;
                };
                let result = alerts::fetch(alerts_config);
                let mut state = lock(&self.state);
                match result {
                    Ok(list) => state.alerts.record(Ok(list)),
                    Err(FetchError::NoToken) => {
                        state.alerts.record_hard_failure(NO_TOKEN.to_owned());
                    }
                    Err(error @ FetchError::Failed(_)) => state.alerts.record(Err(error.reason())),
                }
            }
            Job::Queue => {
                let Some(url) = &self.config.ci.prometheus_url else {
                    return;
                };
                let result = prometheus::fetch(url).map_err(describe);
                lock(&self.state).queue.record(result);
            }
            Job::Pool => {
                let Some(zfs_config) = &self.config.zfs else {
                    return;
                };
                let labels = disk_labels(&self.config);
                let label = |path: &str| {
                    block_device_of(Path::new(path)).and_then(|disk| labels.get(&disk).cloned())
                };
                let result = zfs_config
                    .pools
                    .iter()
                    .map(|pool| zfs::read_pool(pool, &label))
                    .collect::<anyhow::Result<Vec<_>>>()
                    .map_err(describe);
                lock(&self.state).pools.record(result);
            }
            Job::Smart => {
                for disk in self.config.disks.iter().filter(|d| d.kind == DiskKind::Hdd) {
                    let result = smart::read(&disk.by_id).map_err(describe);
                    lock(&self.state)
                        .smart
                        .entry(disk.by_id.to_string_lossy().into_owned())
                        .or_default()
                        .record(result);
                }
            }
        }
    }
}

fn worker_loop(mut worker: Worker, stop: Arc<AtomicBool>) {
    let mut due = [Instant::now(); JOBS.len()];
    while !stop.load(Ordering::Relaxed) {
        for (job, next) in JOBS.iter().zip(due.iter_mut()) {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if Instant::now() < *next || !worker.applies(*job) {
                continue;
            }
            // A panicking source must not take the thread (and every other
            // source) down with it.
            let _ = catch_unwind(AssertUnwindSafe(|| worker.run(*job)));
            *next = Instant::now() + job.interval();
        }
        thread::sleep(WORKER_POLL);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Long-running collector: 1 Hz sources sampled by the caller's thread, slow
/// sources refreshed by a background thread and merged into every snapshot.
pub struct Collector {
    config: Config,
    fast: Fast,
    state: Arc<Mutex<SlowState>>,
    stop: Arc<AtomicBool>,
}

impl Collector {
    /// Spawns the background thread for slow sources.
    pub fn start(config: &Config) -> Self {
        let state = Arc::new(Mutex::new(SlowState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker::new(config, Arc::clone(&state));
        let thread_stop = Arc::clone(&stop);
        if let Err(error) = thread::Builder::new()
            .name("aoostar-lcd-slow".to_owned())
            .spawn(move || worker_loop(worker, thread_stop))
        {
            // Slow sources then stay "pending"; fast sources keep working.
            eprintln!("aoostar-lcd: cannot start background collector: {error}");
        }
        Self {
            config: config.clone(),
            fast: Fast::new(),
            state,
            stop,
        }
    }

    /// Samples fast sources (rates against the previous call) and merges the
    /// latest slow-source state. Called once per second.
    pub fn snapshot(&mut self) -> Snapshot {
        let sample = self.fast.sample(&self.config, false);
        assemble(&self.config, sample, &self.state)
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// One complete synchronous pass: two fast samples one second apart and every
/// slow source once. Used by `aoostar-lcd snapshot` and `aoostar-lcd check`.
pub fn collect_once(config: &Config) -> Snapshot {
    let started = Instant::now();
    let mut fast = Fast::new();
    fast.sample(config, true);

    let state = Arc::new(Mutex::new(SlowState::default()));
    let mut worker = Worker::new(config, Arc::clone(&state));
    for job in JOBS {
        if worker.applies(job) {
            worker.run(job);
        }
    }

    // Rates need at least a second between the two fast samples.
    if let Some(remaining) = Duration::from_secs(1).checked_sub(started.elapsed()) {
        thread::sleep(remaining);
    }
    let sample = fast.sample(config, true);
    assemble(config, sample, &state)
}

/// Merges a fast sample with the current slow state into a snapshot.
fn assemble(config: &Config, sample: Sample, state: &Mutex<SlowState>) -> Snapshot {
    let now = Instant::now();
    let slow = lock(state);

    let docker = slow.docker.fresh(now, Job::Docker.interval());
    let systemd = slow.systemd.fresh(now, Job::Systemd.interval());
    let pools = slow.pools.fresh(now, Job::Pool.interval());
    let queue = slow.queue.fresh(now, Job::Queue.interval());

    let mut disks = sample.disks;
    for (disk, disk_config) in disks.iter_mut().zip(&config.disks) {
        if disk_config.kind != DiskKind::Hdd || !disk.present {
            continue;
        }
        let key = disk_config.by_id.to_string_lossy();
        if let Some(info) = slow
            .smart
            .get(&*key)
            .and_then(|cached| cached.fresh(now, Job::Smart.interval()))
        {
            disk.temp_c = info.temp_c;
            disk.self_test = info.self_test.clone();
            disk.smart_failed = info.failed;
        }
    }

    let alerts_input = match &config.alerts {
        None => AlertsInput::Disabled,
        Some(_) => match slow.alerts.fresh(now, Job::Alerts.interval()) {
            Some(list) => AlertsInput::Active(list),
            None => match slow.alerts.error() {
                Some(NO_TOKEN) => AlertsInput::NoToken,
                Some(reason) => AlertsInput::Unavailable(reason),
                None if slow.alerts.has_value() => AlertsInput::Unavailable("stale"),
                None => AlertsInput::Pending,
            },
        },
    };
    let no_families: &[String] = &[];
    let (local_node, outage_families) = match &config.alerts {
        Some(alerts) => (
            alerts.local_node.as_deref(),
            alerts.outage_families.as_slice(),
        ),
        None => (None, no_families),
    };
    let health = health::build(&Inputs {
        alerts: alerts_input,
        local_node,
        outage_families,
        systemd,
        docker,
        pools: pools.map_or(&[], Vec::as_slice),
    });

    let no_names = HashMap::new();
    let (top_cpu, top_memory) =
        cgroups::top(&sample.workloads, docker.map_or(&no_names, |d| &d.names));

    let services = systemd
        .map(|state| {
            let mut services: Vec<ServiceStatus> = state
                .stacks
                .iter()
                .chain(state.units.iter())
                .map(|unit| ServiceStatus {
                    subject: unit.subject.clone(),
                    active: unit.is_active(),
                })
                .collect();
            // Deterministic order: the rotation compares plans every tick.
            services.sort_by(|a, b| a.subject.cmp(&b.subject));
            services
        })
        .unwrap_or_default();

    let hwmon = sample.hwmon;
    Snapshot {
        taken_at: unix_now(),
        host: Host {
            cpu_percent: sample.cpu_percent,
            load1: sample.load1,
            cpu_temp_c: hwmon.cpu_temp_c,
            ram_temp_c: hwmon.ram_temp_c,
            mem_total: sample.mem.total,
            mem_available: sample.mem.available,
            swap_total: sample.mem.swap_total,
            swap_used: sample.mem.swap_total.saturating_sub(sample.mem.swap_free),
            gpu: slow.gpu.fresh(now, Job::Gpu.interval()).cloned(),
            fans: hwmon.fans,
            uptime_secs: sample.uptime_secs,
        },
        disks,
        volumes: sample.volumes,
        pools: pools.cloned().unwrap_or_default(),
        datasets: sample.datasets,
        top_cpu,
        top_memory,
        services,
        ci: Ci {
            available: docker.is_some(),
            running: docker.map(|d| d.pipelines.clone()).unwrap_or_default(),
            queued: queue.and_then(|q| q.queued),
            oldest_queued_secs: queue.and_then(|q| q.oldest_secs),
        },
        health,
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
    let device = fs::canonicalize(path).ok()?;
    let name = device.file_name()?.to_string_lossy().into_owned();
    let sys = Path::new("/sys/class/block").join(&name);
    if sys.join("partition").exists() {
        let parent = fs::canonicalize(&sys).ok()?;
        return Some(parent.parent()?.file_name()?.to_string_lossy().into_owned());
    }
    Some(name)
}
