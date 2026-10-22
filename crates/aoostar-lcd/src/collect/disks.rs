//! Per-disk I/O rates from /proc/diskstats and NVMe temperatures from sysfs.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::DiskConfig;
use crate::model::{Disk, DiskKind};

use super::host::read_temp;

const SECTOR_BYTES: u64 = 512;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoCounters {
    pub reads: u64,
    pub read_sectors: u64,
    pub writes: u64,
    pub write_sectors: u64,
    /// Milliseconds the device had I/O in flight.
    pub io_ticks_ms: u64,
}

/// `name -> counters` for every device in /proc/diskstats.
pub fn parse_diskstats(text: &str) -> HashMap<String, IoCounters> {
    let mut devices = HashMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(_major), Some(_minor), Some(name)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let fields: Vec<u64> = parts.map(|field| field.parse().unwrap_or(0)).collect();
        // reads, merged, sectors read, ms reading, writes, merged, sectors
        // written, ms writing, in flight, io_ticks, ...
        if fields.len() < 10 {
            continue;
        }
        devices.insert(
            name.to_owned(),
            IoCounters {
                reads: fields[0],
                read_sectors: fields[2],
                writes: fields[4],
                write_sectors: fields[6],
                io_ticks_ms: fields[9],
            },
        );
    }
    devices
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    pub read_bps: u64,
    pub write_bps: u64,
    pub read_iops: u64,
    pub write_iops: u64,
    pub busy_percent: f32,
}

pub fn rates(previous: IoCounters, current: IoCounters, elapsed: Duration) -> Rates {
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return Rates::default();
    }
    let per_second = |delta: u64| (delta as f64 / seconds).round() as u64;
    let busy = current.io_ticks_ms.saturating_sub(previous.io_ticks_ms) as f64 / (seconds * 1000.0)
        * 100.0;
    Rates {
        read_bps: per_second(
            current
                .read_sectors
                .saturating_sub(previous.read_sectors)
                .saturating_mul(SECTOR_BYTES),
        ),
        write_bps: per_second(
            current
                .write_sectors
                .saturating_sub(previous.write_sectors)
                .saturating_mul(SECTOR_BYTES),
        ),
        read_iops: per_second(current.reads.saturating_sub(previous.reads)),
        write_iops: per_second(current.writes.saturating_sub(previous.writes)),
        busy_percent: busy.clamp(0.0, 100.0) as f32,
    }
}

struct Previous {
    device: String,
    counters: IoCounters,
    at: Instant,
}

#[derive(Default)]
pub struct DiskSampler {
    previous: HashMap<String, Previous>,
}

impl DiskSampler {
    /// One entry per configured disk, in configuration order. NVMe
    /// temperatures come from sysfs; HDD temperatures are filled in later from
    /// the slow smartctl source.
    pub fn sample(&mut self, configs: &[DiskConfig], diskstats: &str, now: Instant) -> Vec<Disk> {
        let stats = parse_diskstats(diskstats);
        configs
            .iter()
            .map(|config| {
                let mut disk = Disk {
                    label: config.label.clone(),
                    kind: config.kind,
                    ..Disk::default()
                };
                let Some(device) = resolve_kernel_name(&config.by_id) else {
                    self.previous.remove(&config.label);
                    return disk;
                };
                disk.present = true;
                if config.kind == DiskKind::Nvme {
                    disk.temp_c = nvme_temp(&device);
                }
                let Some(current) = stats.get(&device).copied() else {
                    self.previous.remove(&config.label);
                    return disk;
                };
                if let Some(previous) = self.previous.get(&config.label)
                    && previous.device == device
                {
                    let rates = rates(
                        previous.counters,
                        current,
                        now.saturating_duration_since(previous.at),
                    );
                    disk.read_bps = rates.read_bps;
                    disk.write_bps = rates.write_bps;
                    disk.read_iops = rates.read_iops;
                    disk.write_iops = rates.write_iops;
                    disk.busy_percent = rates.busy_percent;
                }
                self.previous.insert(
                    config.label.clone(),
                    Previous {
                        device,
                        counters: current,
                        at: now,
                    },
                );
                disk
            })
            .collect()
    }
}

/// Kernel device name (`sda`, `nvme0n1`) behind a by-id symlink.
fn resolve_kernel_name(by_id: &Path) -> Option<String> {
    let target = fs::canonicalize(by_id).ok()?;
    Some(target.file_name()?.to_str()?.to_owned())
}

/// Composite temperature of the controller behind an NVMe namespace.
fn nvme_temp(namespace: &str) -> Option<f32> {
    let device = Path::new("/sys/block").join(namespace).join("device");
    let mut hwmons: Vec<_> = fs::read_dir(&device)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("hwmon"))
        .map(|entry| entry.path())
        .collect();
    hwmons.sort();
    hwmons
        .iter()
        .find_map(|hwmon| read_temp(&hwmon.join("temp1_input")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISKSTATS: &str = "\
 259       0 nvme0n1 100 5 2000 30 50 2 1000 20 0 400 50 0 0 0 0 0 0
 259       1 nvme0n1p1 10 0 80 3 5 0 40 2 0 20 5 0 0 0 0 0 0
   8       0 sda 7 1 64 4 3 0 24 2 0 9 6
   7       0 loop0 1 0 2 0 0 0 0 0 0 0 0
 bogus line
";

    #[test]
    fn diskstats_fields() {
        let stats = parse_diskstats(DISKSTATS);
        assert_eq!(
            stats["nvme0n1"],
            IoCounters {
                reads: 100,
                read_sectors: 2000,
                writes: 50,
                write_sectors: 1000,
                io_ticks_ms: 400
            }
        );
        assert_eq!(stats["sda"].io_ticks_ms, 9);
        assert!(stats.contains_key("loop0"));
        assert!(!stats.contains_key("bogus"));
    }

    #[test]
    fn rates_over_two_seconds() {
        let a = IoCounters {
            reads: 10,
            read_sectors: 100,
            writes: 20,
            write_sectors: 200,
            io_ticks_ms: 1000,
        };
        let b = IoCounters {
            reads: 30,
            read_sectors: 100 + 2048,
            writes: 60,
            write_sectors: 200 + 4096,
            io_ticks_ms: 1000 + 500,
        };
        let r = rates(a, b, Duration::from_secs(2));
        assert_eq!(r.read_bps, 2048 * 512 / 2);
        assert_eq!(r.write_bps, 4096 * 512 / 2);
        assert_eq!(r.read_iops, 10);
        assert_eq!(r.write_iops, 20);
        assert!((r.busy_percent - 25.0).abs() < 0.01);
    }

    #[test]
    fn busy_is_clamped_and_counter_reset_is_safe() {
        let a = IoCounters {
            io_ticks_ms: 0,
            ..IoCounters::default()
        };
        let b = IoCounters {
            io_ticks_ms: 5000,
            ..IoCounters::default()
        };
        assert_eq!(rates(a, b, Duration::from_secs(1)).busy_percent, 100.0);
        assert_eq!(rates(b, a, Duration::from_secs(1)), Rates::default());
        assert_eq!(rates(a, b, Duration::ZERO), Rates::default());
    }
}
