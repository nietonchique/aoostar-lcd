//! Per-workload CPU and memory from cgroup v2: Docker containers
//! (`system.slice/docker-<id>.scope`) and systemd services.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use crate::model::{WorkloadCpu, WorkloadMemory};

const TOP: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// Full 64-hex Docker container id.
    Container(String),
    /// Unit stem without `.service`.
    Service(String),
}

#[derive(Debug, Clone)]
pub struct Workload {
    pub key: Key,
    /// Share of the whole host, 0..=100.
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

/// Maps a directory name inside a slice to a workload key.
pub fn classify(dir_name: &str) -> Option<Key> {
    if let Some(id) = dir_name
        .strip_prefix("docker-")
        .and_then(|rest| rest.strip_suffix(".scope"))
    {
        let is_id = id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
        return is_id.then(|| Key::Container(id.to_owned()));
    }
    let stem = dir_name.strip_suffix(".service")?;
    (!stem.is_empty()).then(|| Key::Service(stem.to_owned()))
}

/// `usage_usec` from cgroup `cpu.stat`.
pub fn parse_cpu_usage(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix("usage_usec "))
        .and_then(|value| value.trim().parse().ok())
}

/// Resident memory without page cache: `anon + shmem` from `memory.stat`.
pub fn parse_memory(text: &str) -> u64 {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            match (parts.next()?, parts.next()?) {
                ("anon" | "shmem", value) => value.parse::<u64>().ok(),
                _ => None,
            }
        })
        .fold(0u64, u64::saturating_add)
}

/// Share of the whole host (all logical CPUs) used between two readings.
pub fn cpu_share(before: Option<u64>, now_usec: u64, elapsed_usec: f64, cpus: usize) -> f32 {
    match before {
        Some(before) if elapsed_usec > 0.0 && cpus > 0 => {
            (now_usec.saturating_sub(before) as f64 / (elapsed_usec * cpus as f64) * 100.0)
                .clamp(0.0, 100.0) as f32
        }
        _ => 0.0,
    }
}

pub struct CgroupSampler {
    root: PathBuf,
    previous: HashMap<String, u64>,
    previous_at: Option<Instant>,
    latest: Vec<Workload>,
}

impl CgroupSampler {
    pub fn new() -> Self {
        Self {
            root: PathBuf::from("/sys/fs/cgroup"),
            previous: HashMap::new(),
            previous_at: None,
            latest: Vec::new(),
        }
    }

    /// Re-reads every workload cgroup. CPU rates are relative to the previous
    /// refresh; the first call reports 0 % everywhere.
    pub fn refresh(&mut self, cpus: usize, now: Instant) -> &[Workload] {
        let elapsed_usec = self.previous_at.map_or(0.0, |at| {
            now.saturating_duration_since(at).as_micros() as f64
        });

        // (path relative to the cgroup root, workload key)
        let mut candidates: Vec<(String, Key)> = Vec::new();
        if let Ok(entries) = fs::read_dir(self.root.join("system.slice")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some(key) = classify(&name) {
                    candidates.push((format!("system.slice/{name}"), key));
                }
            }
        }
        // Interactive logins and everything they start count as one workload.
        candidates.push(("user.slice".to_owned(), Key::Service("user".to_owned())));

        let mut usage = HashMap::new();
        let mut workloads = Vec::new();
        for (relative, key) in candidates {
            let dir = self.root.join(&relative);
            let Some(used_usec) = fs::read_to_string(dir.join("cpu.stat"))
                .ok()
                .as_deref()
                .and_then(parse_cpu_usage)
            else {
                continue;
            };
            let memory_bytes =
                fs::read_to_string(dir.join("memory.stat")).map_or(0, |text| parse_memory(&text));
            let cpu_percent = cpu_share(
                self.previous.get(&relative).copied(),
                used_usec,
                elapsed_usec,
                cpus,
            );
            usage.insert(relative, used_usec);
            workloads.push(Workload {
                key,
                cpu_percent,
                memory_bytes,
            });
        }

        self.previous = usage;
        self.previous_at = Some(now);
        self.latest = workloads;
        &self.latest
    }

    /// Result of the most recent [`refresh`](Self::refresh).
    pub fn latest(&self) -> &[Workload] {
        &self.latest
    }
}

/// Display names, merged by equal name, then the top consumers.
/// `names` maps a Docker container id to its display name.
pub fn top(
    workloads: &[Workload],
    names: &HashMap<String, String>,
) -> (Vec<WorkloadCpu>, Vec<WorkloadMemory>) {
    let mut merged: HashMap<String, (f32, u64)> = HashMap::new();
    for workload in workloads {
        let name = match &workload.key {
            Key::Container(id) => names
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.chars().take(12).collect()),
            Key::Service(stem) => stem.clone(),
        };
        let slot = merged.entry(name).or_default();
        slot.0 += workload.cpu_percent;
        slot.1 = slot.1.saturating_add(workload.memory_bytes);
    }

    let mut cpu: Vec<WorkloadCpu> = merged
        .iter()
        .filter(|(_, (percent, _))| *percent > 0.0)
        .map(|(name, (percent, _))| WorkloadCpu {
            name: name.clone(),
            percent: percent.min(100.0),
        })
        .collect();
    cpu.sort_by(|a, b| {
        b.percent
            .total_cmp(&a.percent)
            .then_with(|| a.name.cmp(&b.name))
    });
    cpu.truncate(TOP);

    let mut memory: Vec<WorkloadMemory> = merged
        .into_iter()
        .filter(|(_, (_, bytes))| *bytes > 0)
        .map(|(name, (_, bytes))| WorkloadMemory { name, bytes })
        .collect();
    memory.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    memory.truncate(TOP);
    (cpu, memory)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01e0874073d3baa542ace0671b1c9f9ff8b2bce882aca47728cc6d9a0ee59e1a";

    #[test]
    fn classifies_docker_scopes_and_services() {
        assert_eq!(
            classify(&format!("docker-{ID}.scope")),
            Some(Key::Container(ID.to_owned()))
        );
        assert_eq!(
            classify("vllm-server.service"),
            Some(Key::Service("vllm-server".to_owned()))
        );
        assert_eq!(classify("docker-abc.scope"), None);
        assert_eq!(classify("init.scope"), None);
        assert_eq!(classify("boot.mount"), None);
        assert_eq!(classify("system-getty.slice"), None);
    }

    #[test]
    fn cpu_share_of_whole_host() {
        // 4 CPUs, 2 s elapsed, 2 CPU-seconds used -> 25 % of the host.
        let share = cpu_share(Some(1_000_000), 3_000_000, 2_000_000.0, 4);
        assert!((share - 25.0).abs() < 0.001);
        assert_eq!(cpu_share(None, 3_000_000, 2_000_000.0, 4), 0.0);
        assert_eq!(cpu_share(Some(5), 1, 2_000_000.0, 4), 0.0);
        assert_eq!(cpu_share(Some(0), 9_000_000_000, 1_000_000.0, 2), 100.0);
    }

    #[test]
    fn cpu_stat_and_memory_stat() {
        let cpu = "usage_usec 123456\nuser_usec 100000\nsystem_usec 23456\nnr_periods 0\n";
        assert_eq!(parse_cpu_usage(cpu), Some(123_456));
        assert_eq!(parse_cpu_usage("nr_periods 0\n"), None);
        let memory = "anon 1000\nfile 5000\nkernel 7\nshmem 24\nfile_mapped 3\nanon_thp 9\n";
        assert_eq!(parse_memory(memory), 1024);
    }

    #[test]
    fn top_merges_equal_names_and_ranks() {
        let other = "f".repeat(64);
        let names: HashMap<String, String> = [
            (ID.to_owned(), "CI l2-web".to_owned()),
            (other.clone(), "CI l2-web".to_owned()),
        ]
        .into();
        let workloads = vec![
            Workload {
                key: Key::Container(ID.to_owned()),
                cpu_percent: 10.0,
                memory_bytes: 100,
            },
            Workload {
                key: Key::Container(other),
                cpu_percent: 15.0,
                memory_bytes: 200,
            },
            Workload {
                key: Key::Service("docker".to_owned()),
                cpu_percent: 20.0,
                memory_bytes: 50,
            },
            Workload {
                key: Key::Container("a".repeat(64)),
                cpu_percent: 0.0,
                memory_bytes: 400,
            },
        ];
        let (cpu, memory) = top(&workloads, &names);
        assert_eq!(cpu.len(), 2);
        assert_eq!(cpu[0].name, "CI l2-web");
        assert!((cpu[0].percent - 25.0).abs() < 0.001);
        assert_eq!(cpu[1].name, "docker");
        // Unknown container ids fall back to the 12-char short id.
        assert_eq!(memory[0].name, "aaaaaaaaaaaa");
        assert_eq!(memory[1].name, "CI l2-web");
        assert_eq!(memory[1].bytes, 300);
    }
}
