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
mod sources;
mod volumes;
mod zfs;

// Crate-visible: the shutdown page reuses these collectors directly.
pub(crate) mod docker;
pub(crate) mod systemd;

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::model::{Ci, DiskKind, Host, ServiceStatus, Snapshot};

use alerts::NO_TOKEN;
use fast::{Fast, Sample};
use health::{AlertsInput, Inputs};
use sources::{Slots, Sources};

const WORKER_POLL: Duration = Duration::from_millis(200);
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
/// A failed source keeps its last good value this many intervals.
pub(crate) const GRACE_INTERVALS: u32 = 3;

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

/// Worker thread: refreshes every enabled adapter when due. Fetch runs
/// without locks; only the short commit touches the shared slots. A
/// panicking adapter must not take the thread (and every other source) down
/// with it.
fn worker_loop(
    mut sources: Sources,
    config: Config,
    slots: Arc<Mutex<Slots>>,
    stop: Arc<AtomicBool>,
) {
    let mut adapters = sources.adapters();
    let mut due = [Instant::now(); 7];
    while !stop.load(Ordering::Relaxed) {
        for (adapter, next) in adapters.iter_mut().zip(due.iter_mut()) {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if Instant::now() < *next || !adapter.applies(&config) {
                continue;
            }
            let _ = catch_unwind(AssertUnwindSafe(|| {
                adapter.fetch(&config);
                adapter.commit(&mut lock(&slots));
            }));
            *next = Instant::now() + adapter.interval();
        }
        thread::sleep(WORKER_POLL);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

fn lock(slots: &Mutex<Slots>) -> MutexGuard<'_, Slots> {
    slots.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Long-running collector: 1 Hz sources sampled by the caller's thread, slow
/// sources refreshed by a background thread and merged into every snapshot.
pub struct Collector {
    config: Config,
    fast: Fast,
    slots: Arc<Mutex<Slots>>,
    stop: Arc<AtomicBool>,
}

impl Collector {
    /// Spawns the background thread for slow sources.
    pub fn start(config: &Config) -> Self {
        let slots = Arc::new(Mutex::new(Slots::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Sources::new(config);
        let thread_slots = Arc::clone(&slots);
        let thread_config = config.clone();
        let thread_stop = Arc::clone(&stop);
        if let Err(error) = thread::Builder::new()
            .name("aoostar-lcd-slow".to_owned())
            .spawn(move || worker_loop(worker, thread_config, thread_slots, thread_stop))
        {
            // Slow sources then stay "pending"; fast sources keep working.
            eprintln!("aoostar-lcd: cannot start background collector: {error}");
        }
        Self {
            config: config.clone(),
            fast: Fast::new(),
            slots,
            stop,
        }
    }

    /// Samples fast sources (rates against the previous call) and merges the
    /// latest slow-source state. Called once per second.
    pub fn snapshot(&mut self) -> Snapshot {
        let sample = self.fast.sample(&self.config, false);
        let slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        assemble(&self.config, sample, &slots)
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

    let mut slots = Slots::default();
    Sources::new(config).refresh_all(config, &mut slots);

    // Rates need at least a second between the two fast samples.
    if let Some(remaining) = Duration::from_secs(1).checked_sub(started.elapsed()) {
        thread::sleep(remaining);
    }
    let sample = fast.sample(config, true);
    assemble(config, sample, &slots)
}

/// Merges a fast sample with the current slow-source slots into a snapshot.
fn assemble(config: &Config, sample: Sample, slots: &Slots) -> Snapshot {
    let now = Instant::now();

    let docker = slots.docker(now);
    let systemd = slots.systemd(now);
    let pools = slots.pools(now);
    let queue = slots.queue(now);

    let mut disks = sample.disks;
    for (disk, disk_config) in disks.iter_mut().zip(&config.disks) {
        if disk_config.kind != DiskKind::Hdd || !disk.present {
            continue;
        }
        let key = disk_config.by_id.to_string_lossy();
        if let Some(info) = slots.smart(&key, now) {
            disk.temp_c = info.temp_c;
            disk.self_test = info.self_test.clone();
            disk.smart_failed = info.failed;
        }
    }

    let alerts_input = match &config.alerts {
        None => AlertsInput::Disabled,
        Some(_) => match slots.alerts(now) {
            Some(list) => AlertsInput::Active(list),
            None => match slots.alerts_error() {
                Some(NO_TOKEN) => AlertsInput::NoToken,
                Some(reason) => AlertsInput::Unavailable(reason),
                None if slots.alerts_ever_fetched() => AlertsInput::Unavailable("stale"),
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
            gpu: slots.gpu(now).cloned(),
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
