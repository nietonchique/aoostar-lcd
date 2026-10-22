//! Everything sampled on the caller's thread once per second: file reads only.

use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::config::Config;
use crate::model::{DatasetIo, Disk, Volume};

use super::cgroups::{CgroupSampler, Workload};
use super::disks::DiskSampler;
use super::host::{self, CpuTimes, Hwmon, MemInfo};
use super::volumes;
use super::zfs::DatasetSampler;

/// The cgroup tree is walked on every second tick.
const CGROUP_EVERY: u64 = 2;

pub struct Sample {
    pub cpu_percent: Option<f32>,
    pub load1: Option<f32>,
    /// Seconds since boot, from `/proc/uptime`.
    pub uptime_secs: Option<u64>,
    pub mem: MemInfo,
    pub hwmon: Hwmon,
    pub disks: Vec<Disk>,
    pub volumes: Vec<Volume>,
    pub datasets: Vec<DatasetIo>,
    pub workloads: Vec<Workload>,
}

pub struct Fast {
    previous_cpu: Option<CpuTimes>,
    disks: DiskSampler,
    datasets: DatasetSampler,
    cgroups: CgroupSampler,
    tick: u64,
}

impl Fast {
    pub fn new() -> Self {
        Self {
            previous_cpu: None,
            disks: DiskSampler::default(),
            datasets: DatasetSampler::default(),
            cgroups: CgroupSampler::new(),
            tick: 0,
        }
    }

    /// Samples every fast source; rates are relative to the previous call.
    /// `force_cgroups` refreshes the cgroup tree regardless of the tick.
    pub fn sample(&mut self, config: &Config, force_cgroups: bool) -> Sample {
        let now = Instant::now();

        let stat = fs::read_to_string("/proc/stat")
            .ok()
            .and_then(|text| host::parse_proc_stat(&text));
        let cpus = stat.map_or(1, |(_, cpus)| cpus);
        let current_cpu = stat.map(|(times, _)| times);
        let cpu_percent = match (self.previous_cpu, current_cpu) {
            (Some(previous), Some(current)) => host::cpu_percent(previous, current),
            _ => None,
        };
        self.previous_cpu = current_cpu;

        let load1 = fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|text| host::parse_loadavg(&text));
        let uptime_secs = fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|text| super::systemd::parse_uptime(&text))
            .map(|uptime| uptime as u64);
        let mem = fs::read_to_string("/proc/meminfo")
            .map(|text| host::parse_meminfo(&text))
            .unwrap_or_default();
        let hwmon = host::read_hwmon(Path::new("/sys/class/hwmon"));

        let diskstats = fs::read_to_string("/proc/diskstats").unwrap_or_default();
        let disks = self.disks.sample(&config.disks, &diskstats, now);

        let mountinfo = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        let volumes = volumes::sample(&config.volumes, &mountinfo);

        let datasets = match &config.zfs {
            Some(zfs) => self.datasets.sample(&zfs.pools, now),
            None => Vec::new(),
        };

        if force_cgroups || self.tick.is_multiple_of(CGROUP_EVERY) {
            self.cgroups.refresh(cpus, now);
        }
        let workloads = self.cgroups.latest().to_vec();
        self.tick += 1;

        Sample {
            cpu_percent,
            load1,
            uptime_secs,
            mem,
            hwmon,
            disks,
            volumes,
            datasets,
            workloads,
        }
    }
}
