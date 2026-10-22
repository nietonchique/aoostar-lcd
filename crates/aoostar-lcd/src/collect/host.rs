//! Whole-host fast sources: /proc/stat, /proc/loadavg, /proc/meminfo, hwmon.

use std::fs;
use std::path::Path;

use crate::model::Fan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    pub total: u64,
    pub idle: u64,
}

/// Aggregate CPU times and the number of logical CPUs from /proc/stat.
pub fn parse_proc_stat(text: &str) -> Option<(CpuTimes, usize)> {
    let mut aggregate = None;
    let mut cpus = 0usize;
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("cpu") else {
            continue;
        };
        if rest.starts_with(|c: char| c.is_ascii_digit()) {
            cpus += 1;
            continue;
        }
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let fields: Vec<u64> = rest
            .split_whitespace()
            .map(|field| field.parse().unwrap_or(0))
            .collect();
        if fields.len() < 4 {
            return None;
        }
        // user nice system idle iowait irq softirq steal; guest time is
        // already part of user/nice.
        let total: u64 = fields.iter().take(8).sum();
        let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
        aggregate = Some(CpuTimes { total, idle });
    }
    aggregate.map(|times| (times, cpus.max(1)))
}

/// Utilisation between two samples, 0..=100; `None` when no time elapsed.
pub fn cpu_percent(previous: CpuTimes, current: CpuTimes) -> Option<f32> {
    let total = current.total.saturating_sub(previous.total);
    if total == 0 {
        return None;
    }
    let idle = current.idle.saturating_sub(previous.idle).min(total);
    Some(((total - idle) as f64 / total as f64 * 100.0) as f32)
}

pub fn parse_loadavg(text: &str) -> Option<f32> {
    text.split_whitespace().next()?.parse().ok()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemInfo {
    pub total: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

/// Values in bytes.
pub fn parse_meminfo(text: &str) -> MemInfo {
    let mut info = MemInfo::default();
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let target = match key {
            "MemTotal" => &mut info.total,
            "MemAvailable" => &mut info.available,
            "SwapTotal" => &mut info.swap_total,
            "SwapFree" => &mut info.swap_free,
            _ => continue,
        };
        let mut parts = rest.split_whitespace();
        let Some(value) = parts.next().and_then(|v| v.parse::<u64>().ok()) else {
            continue;
        };
        let multiplier = match parts.next() {
            Some("kB") => 1024,
            _ => 1,
        };
        *target = value.saturating_mul(multiplier);
    }
    info
}

#[derive(Debug, Clone, Default)]
pub struct Hwmon {
    pub cpu_temp_c: Option<f32>,
    pub ram_temp_c: Option<f32>,
    pub fans: Vec<Fan>,
}

/// CPU, DIMM and fan sensors from `/sys/class/hwmon`.
pub fn read_hwmon(root: &Path) -> Hwmon {
    let mut result = Hwmon::default();
    let Ok(entries) = fs::read_dir(root) else {
        return result;
    };
    let mut dirs: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    dirs.sort();
    for dir in dirs {
        let Ok(name) = fs::read_to_string(dir.join("name")) else {
            continue;
        };
        match name.trim() {
            "k10temp" => {
                result.cpu_temp_c = read_cpu_tctl(&dir).or(result.cpu_temp_c);
            }
            "spd5118" => {
                if let Some(temp) = read_temp(&dir.join("temp1_input")) {
                    result.ram_temp_c = Some(result.ram_temp_c.map_or(temp, |old| old.max(temp)));
                }
            }
            chip if is_super_io(chip) => {
                for index in 1..=8u8 {
                    let rpm = fs::read_to_string(dir.join(format!("fan{index}_input")))
                        .ok()
                        .and_then(|text| text.trim().parse::<u32>().ok())
                        .unwrap_or(0);
                    if rpm > 0 {
                        result.fans.push(Fan {
                            label: fan_label(index),
                            rpm,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    result
}

/// `Tctl` is the control temperature k10temp exposes as `temp1`.
fn read_cpu_tctl(dir: &Path) -> Option<f32> {
    for index in 1..=4u8 {
        let label = fs::read_to_string(dir.join(format!("temp{index}_label"))).ok();
        if label.is_some_and(|label| label.trim() == "Tctl") {
            return read_temp(&dir.join(format!("temp{index}_input")));
        }
    }
    read_temp(&dir.join("temp1_input"))
}

fn is_super_io(chip: &str) -> bool {
    let digits_after = |prefix: &str| {
        chip.strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
    };
    digits_after("it") || chip.starts_with("nct")
}

pub fn fan_label(index: u8) -> String {
    format!("F{index}")
}

/// Reads a hwmon `tempN_input` file (millidegrees Celsius).
pub fn read_temp(path: &Path) -> Option<f32> {
    let milli: i64 = fs::read_to_string(path).ok()?.trim().parse().ok()?;
    Some(milli as f32 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  4705 150 3034 362409 140 10 25 5 0 0\n\
cpu0 1393 75 1019 90601 42 5 12 2 0 0\n\
cpu1 1260 25 800 90600 30 2 6 1 0 0\n\
intr 12345 0 0\n\
ctxt 99999\n";

    #[test]
    fn proc_stat_totals_and_cpu_count() {
        let (times, cpus) = parse_proc_stat(STAT).unwrap();
        assert_eq!(cpus, 2);
        assert_eq!(times.total, 4705 + 150 + 3034 + 362409 + 140 + 10 + 25 + 5);
        assert_eq!(times.idle, 362409 + 140);
    }

    #[test]
    fn proc_stat_without_aggregate_line_is_none() {
        assert!(parse_proc_stat("cpu0 1 2 3 4\nintr 1\n").is_none());
        assert!(parse_proc_stat("").is_none());
    }

    #[test]
    fn cpu_percent_from_deltas() {
        let a = CpuTimes {
            total: 1000,
            idle: 900,
        };
        let b = CpuTimes {
            total: 1200,
            idle: 950,
        };
        let percent = cpu_percent(a, b).unwrap();
        assert!((percent - 75.0).abs() < 0.01);
        assert!(cpu_percent(b, b).is_none());
        // Counters going backwards must not underflow.
        assert!(cpu_percent(b, a).is_none());
    }

    #[test]
    fn loadavg_first_field() {
        assert_eq!(parse_loadavg("0.52 0.58 0.59 1/1234 56789\n"), Some(0.52));
        assert_eq!(parse_loadavg(""), None);
    }

    #[test]
    fn meminfo_values_in_bytes() {
        let text = "MemTotal:       98765432 kB\n\
MemFree:         1000 kB\n\
MemAvailable:   50000000 kB\n\
SwapTotal:       2097148 kB\n\
SwapFree:        2000000 kB\n\
HugePages_Total:       0\n";
        let info = parse_meminfo(text);
        assert_eq!(info.total, 98_765_432 * 1024);
        assert_eq!(info.available, 50_000_000 * 1024);
        assert_eq!(info.swap_total, 2_097_148 * 1024);
        assert_eq!(info.swap_free, 2_000_000 * 1024);
    }

    #[test]
    fn fan_labels_and_chip_matching() {
        assert_eq!(fan_label(2), "F2");
        assert!(is_super_io("it8620"));
        assert!(is_super_io("nct6798"));
        assert!(!is_super_io("nvme"));
        assert!(!is_super_io("item"));
    }
}
