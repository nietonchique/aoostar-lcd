//! ZFS: per-dataset logical I/O (fast, /proc/spl kstats) and pool health /
//! scrub progress (slow, `zpool`).

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use serde_json::Value;

use crate::model::{BadDevice, DatasetIo, Pool, PoolScan, ScanKind};

use super::proc;

const TOP_DATASETS: usize = 5;
const ZPOOL_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Dataset I/O
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Objset {
    pub name: String,
    pub nread: u64,
    pub nwritten: u64,
}

/// Parses one `/proc/spl/kstat/zfs/<pool>/objset-0x*` file.
pub fn parse_objset(text: &str) -> Option<Objset> {
    let mut name = None;
    let mut nread = None;
    let mut nwritten = None;
    for line in text.lines() {
        let mut columns = line.split_whitespace();
        let (Some(key), Some(_type), Some(value)) =
            (columns.next(), columns.next(), columns.next())
        else {
            continue;
        };
        match key {
            "dataset_name" => name = Some(value.to_owned()),
            "nread" => nread = value.parse().ok(),
            "nwritten" => nwritten = value.parse().ok(),
            _ => {}
        }
    }
    Some(Objset {
        name: name?,
        nread: nread?,
        nwritten: nwritten?,
    })
}

/// `storage01/backups/urbackup` -> `backups/urbackup`; the pool root keeps the
/// pool name.
pub fn strip_pool(pool: &str, dataset: &str) -> String {
    match dataset
        .strip_prefix(pool)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        Some(rest) if !rest.is_empty() => rest.to_owned(),
        _ => dataset.to_owned(),
    }
}

#[derive(Default)]
pub struct DatasetSampler {
    previous: HashMap<String, (u64, u64)>,
    at: Option<Instant>,
}

impl DatasetSampler {
    /// Busiest datasets (by read + write bytes/s) across `pools` since the
    /// previous call. The first pool is primary: its prefix is stripped.
    pub fn sample(&mut self, pools: &[String], now: Instant) -> Vec<DatasetIo> {
        let mut current: HashMap<String, (u64, u64)> = HashMap::new();
        for pool in pools {
            let directory = Path::new("/proc/spl/kstat/zfs").join(pool);
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                if !entry.file_name().to_string_lossy().starts_with("objset-") {
                    continue;
                }
                let Ok(text) = fs::read_to_string(entry.path()) else {
                    continue;
                };
                if let Some(objset) = parse_objset(&text) {
                    let slot = current.entry(objset.name).or_default();
                    slot.0 = slot.0.saturating_add(objset.nread);
                    slot.1 = slot.1.saturating_add(objset.nwritten);
                }
            }
        }
        let primary = pools.first().map_or("", String::as_str);
        let datasets = match self.at {
            Some(at) => dataset_rates(
                primary,
                &self.previous,
                &current,
                now.saturating_duration_since(at),
            ),
            None => Vec::new(),
        };
        self.previous = current;
        self.at = Some(now);
        datasets
    }
}

pub fn dataset_rates(
    pool: &str,
    previous: &HashMap<String, (u64, u64)>,
    current: &HashMap<String, (u64, u64)>,
    elapsed: Duration,
) -> Vec<DatasetIo> {
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return Vec::new();
    }
    let mut rows: Vec<DatasetIo> = current
        .iter()
        .filter_map(|(name, &(read, written))| {
            let &(old_read, old_written) = previous.get(name)?;
            let read_bps = (read.saturating_sub(old_read) as f64 / seconds).round() as u64;
            let write_bps = (written.saturating_sub(old_written) as f64 / seconds).round() as u64;
            (read_bps > 0 || write_bps > 0).then(|| DatasetIo {
                name: strip_pool(pool, name),
                read_bps,
                write_bps,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        (b.read_bps.saturating_add(b.write_bps))
            .cmp(&a.read_bps.saturating_add(a.write_bps))
            .then_with(|| a.name.cmp(&b.name))
    });
    rows.truncate(TOP_DATASETS);
    rows
}

// ---------------------------------------------------------------------------
// Pool status
// ---------------------------------------------------------------------------

/// Runs `zpool status -j -p` and `zpool list` for `pool`; `label` maps a leaf
/// vdev device path to the configured disk label.
pub fn read_pool(pool: &str, label: &dyn Fn(&str) -> Option<String>) -> anyhow::Result<Pool> {
    let status =
        proc::run("zpool", &["status", "-j", "-p", pool], ZPOOL_TIMEOUT).context("zpool status")?;
    if !status.success {
        bail!("zpool status failed");
    }
    let list = proc::run(
        "zpool",
        &["list", "-Hp", "-o", "size,alloc,health", pool],
        ZPOOL_TIMEOUT,
    )
    .context("zpool list")?;
    if !list.success {
        bail!("zpool list failed");
    }
    parse_pool(pool, &status.stdout, &list.stdout, super::unix_now(), label)
}

/// Numbers in `zpool -j -p` output are strings; accept both forms.
fn number(value: &Value) -> Option<u64> {
    match value {
        Value::String(text) => text.trim().parse().ok(),
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

fn field(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(number)
}

/// Sum of read/write/checksum errors over leaf vdevs.
fn leaf_errors(vdevs: &Value) -> u64 {
    let Some(map) = vdevs.as_object() else {
        return 0;
    };
    map.values()
        .map(|vdev| match vdev.get("vdevs").and_then(Value::as_object) {
            Some(children) if !children.is_empty() => leaf_errors(&vdev["vdevs"]),
            _ => ["read_errors", "write_errors", "checksum_errors"]
                .iter()
                .filter_map(|key| field(vdev, key))
                .fold(0u64, u64::saturating_add),
        })
        .fold(0u64, u64::saturating_add)
}

/// Leaf vdevs that are not `ONLINE`: `(name, path, state)`.
fn bad_leaves(vdevs: &Value, out: &mut Vec<(String, String, String)>) {
    let Some(map) = vdevs.as_object() else {
        return;
    };
    for (key, vdev) in map {
        match vdev.get("vdevs").and_then(Value::as_object) {
            Some(children) if !children.is_empty() => bad_leaves(&vdev["vdevs"], out),
            _ => {
                let state = vdev
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("UNKNOWN");
                if state != "ONLINE" {
                    let text =
                        |field: &str| vdev.get(field).and_then(Value::as_str).map(str::to_owned);
                    out.push((
                        text("name").unwrap_or_else(|| key.clone()),
                        text("path").unwrap_or_default(),
                        state.to_owned(),
                    ));
                }
            }
        }
    }
}

pub fn parse_pool(
    name: &str,
    status_json: &str,
    list_output: &str,
    now: i64,
    label: &dyn Fn(&str) -> Option<String>,
) -> anyhow::Result<Pool> {
    let root: Value = serde_json::from_str(status_json).context("zpool status json")?;
    let pool = root
        .pointer(&format!("/pools/{name}"))
        .ok_or_else(|| anyhow!("pool {name} missing in zpool status"))?;

    let mut columns = list_output.split_whitespace();
    let total = columns.next().and_then(|v| v.parse::<u64>().ok());
    let used = columns.next().and_then(|v| v.parse::<u64>().ok());
    let list_health = columns.next();

    let health = pool
        .get("state")
        .and_then(Value::as_str)
        .or(list_health)
        .ok_or_else(|| anyhow!("pool health unknown"))?
        .to_owned();

    let mut bad = Vec::new();
    if let Some(vdevs) = pool.get("vdevs") {
        bad_leaves(vdevs, &mut bad);
    }
    let bad_devices = bad
        .into_iter()
        .map(|(vdev, path, state)| BadDevice {
            name: label(&path).unwrap_or(vdev),
            state,
        })
        .collect();

    Ok(Pool {
        name: name.to_owned(),
        health,
        used: used.unwrap_or(0),
        total: total.unwrap_or(0),
        errors: pool.get("vdevs").map_or(0, leaf_errors),
        bad_devices,
        scan: pool
            .get("scan_stats")
            .and_then(|scan| parse_scan(scan, now)),
    })
}

/// A scan that is currently running (or paused); finished/canceled scans are
/// not shown.
fn parse_scan(scan: &Value, now: i64) -> Option<PoolScan> {
    let kind = match scan.get("function").and_then(Value::as_str)? {
        "SCRUB" => ScanKind::Scrub,
        "RESILVER" => ScanKind::Resilver,
        _ => return None,
    };
    if scan.get("state").and_then(Value::as_str)? != "SCANNING" {
        return None;
    }
    let to_examine = field(scan, "to_examine").unwrap_or(0);
    let issued = field(scan, "issued").unwrap_or(0);
    let paused = field(scan, "scrub_pause").unwrap_or(0) != 0;
    let percent = if to_examine == 0 {
        0.0
    } else {
        (issued as f64 / to_examine as f64 * 100.0).clamp(0.0, 100.0) as f32
    };

    let eta_secs = if paused || issued == 0 {
        None
    } else {
        let pass_start = field(scan, "pass_start").unwrap_or(0) as i64;
        let spent_paused = field(scan, "scrub_spent_paused").unwrap_or(0) as i64;
        let elapsed = now - pass_start - spent_paused;
        if pass_start > 0 && elapsed > 0 {
            let rate = issued as f64 / elapsed as f64;
            let remaining = to_examine.saturating_sub(issued) as f64;
            Some((remaining / rate).round() as u64)
        } else {
            None
        }
    };

    Some(PoolScan {
        kind,
        paused,
        percent,
        eta_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJSET: &str = "\
132 1 0x01 28 7872 22849687029 219240688842196
name                            type data
dataset_name                    7    storage01/media/music
writes                          4    3
nwritten                        4    4096
reads                           4    4
nread                           4    106496
nunlinks                        4    0
";

    #[test]
    fn objset_fields() {
        let objset = parse_objset(OBJSET).unwrap();
        assert_eq!(
            objset,
            Objset {
                name: "storage01/media/music".into(),
                nread: 106_496,
                nwritten: 4096
            }
        );
        assert!(parse_objset("name type data\n").is_none());
    }

    #[test]
    fn pool_prefix_is_stripped() {
        assert_eq!(
            strip_pool("storage01", "storage01/backups/urbackup"),
            "backups/urbackup"
        );
        assert_eq!(strip_pool("storage01", "storage01"), "storage01");
        assert_eq!(strip_pool("storage01", "storage010/x"), "storage010/x");
    }

    #[test]
    fn busiest_datasets_with_traffic_only() {
        let previous: HashMap<String, (u64, u64)> = [
            ("p/a".to_owned(), (0, 0)),
            ("p/b".to_owned(), (0, 0)),
            ("p/idle".to_owned(), (5, 5)),
            ("p".to_owned(), (0, 0)),
        ]
        .into();
        let current: HashMap<String, (u64, u64)> = [
            ("p/a".to_owned(), (2000, 0)),
            ("p/b".to_owned(), (0, 6000)),
            ("p/idle".to_owned(), (5, 5)),
            ("p/new".to_owned(), (9_999_999, 0)),
            ("p".to_owned(), (100, 0)),
        ]
        .into();
        let rows = dataset_rates("p", &previous, &current, Duration::from_secs(2));
        let names: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();
        // p/new has no previous sample, p/idle has no traffic.
        assert_eq!(names, ["b", "a", "p"]);
        assert_eq!(rows[0].write_bps, 3000);
        assert_eq!(rows[1].read_bps, 1000);
    }

    #[test]
    fn dataset_list_is_truncated_to_five() {
        let previous: HashMap<String, (u64, u64)> =
            (0..8).map(|i| (format!("p/d{i}"), (0, 0))).collect();
        let current: HashMap<String, (u64, u64)> = (0..8)
            .map(|i| (format!("p/d{i}"), (100 * (i + 1), 0)))
            .collect();
        let rows = dataset_rates("p", &previous, &current, Duration::from_secs(1));
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].name, "d7");
    }

    const FINISHED: &str = r#"{"output_version":{"command":"zpool status","vers_major":0,"vers_minor":1},
"pools":{"storage01":{"name":"storage01","state":"ONLINE","pool_guid":"1","txg":"1614244",
"scan_stats":{"function":"SCRUB","state":"FINISHED","start_time":"1790100000","end_time":"1790360840",
"to_examine":"3650559651840","examined":"3664402362368","pass_start":"1792141836","scrub_pause":"0",
"scrub_spent_paused":"0","issued":"3634624241664"},
"vdevs":{"storage01":{"name":"storage01","vdev_type":"root","state":"ONLINE","read_errors":"0","write_errors":"0","checksum_errors":"0",
"vdevs":{"mirror-0":{"name":"mirror-0","vdev_type":"mirror","state":"ONLINE","read_errors":"0","write_errors":"0","checksum_errors":"0",
"vdevs":{"ata-A":{"name":"ata-A","vdev_type":"disk","state":"ONLINE","read_errors":"0","write_errors":"0","checksum_errors":"0"},
"ata-B":{"name":"ata-B","vdev_type":"disk","state":"ONLINE","read_errors":"0","write_errors":"0","checksum_errors":"0"}}}}}},
"error_count":"0"}}}"#;

    const LIST: &str = "13984413515776\t5761973223424\tONLINE\n";

    #[test]
    fn finished_scrub_is_not_reported() {
        let pool = parse_pool("storage01", FINISHED, LIST, 1_792_300_000, &|_| None).unwrap();
        assert!(pool.bad_devices.is_empty());
        assert_eq!(pool.health, "ONLINE");
        assert_eq!(pool.total, 13_984_413_515_776);
        assert_eq!(pool.used, 5_761_973_223_424);
        assert_eq!(pool.errors, 0);
        assert!(pool.scan.is_none());
    }

    #[test]
    fn scanning_scrub_has_percent_and_eta() {
        let json = FINISHED
            .replace("\"state\":\"FINISHED\"", "\"state\":\"SCANNING\"")
            .replace("\"issued\":\"3634624241664\"", "\"issued\":\"1000000\"")
            .replace(
                "\"to_examine\":\"3650559651840\"",
                "\"to_examine\":\"4000000\"",
            );
        // 1000 s elapsed since pass_start -> 1000 B/s -> 3000 s left.
        let pool = parse_pool("storage01", &json, LIST, 1_792_141_836 + 1000, &|_| None).unwrap();
        let scan = pool.scan.unwrap();
        assert_eq!(scan.kind, ScanKind::Scrub);
        assert!(!scan.paused);
        assert!((scan.percent - 25.0).abs() < 0.001);
        assert_eq!(scan.eta_secs, Some(3000));
    }

    #[test]
    fn paused_resilver_has_no_eta_and_errors_are_summed() {
        let json = FINISHED
            .replace("\"function\":\"SCRUB\"", "\"function\":\"RESILVER\"")
            .replace("\"state\":\"FINISHED\"", "\"state\":\"SCANNING\"")
            .replace("\"scrub_pause\":\"0\"", "\"scrub_pause\":\"1792200000\"")
            .replace(
                "\"name\":\"ata-B\",\"vdev_type\":\"disk\",\"state\":\"ONLINE\",\"read_errors\":\"0\",\"write_errors\":\"0\",\"checksum_errors\":\"0\"",
                "\"name\":\"ata-B\",\"vdev_type\":\"disk\",\"path\":\"/dev/disk/by-id/ata-B-part1\",\"state\":\"FAULTED\",\"read_errors\":\"2\",\"write_errors\":\"0\",\"checksum_errors\":\"5\"",
            )
            .replace("\"state\":\"ONLINE\",\"pool_guid\"", "\"state\":\"DEGRADED\",\"pool_guid\"");
        let label = |path: &str| (path == "/dev/disk/by-id/ata-B-part1").then(|| "H2".to_owned());
        let pool = parse_pool(
            "storage01",
            &json,
            "100\t50\tDEGRADED\n",
            1_792_300_000,
            &label,
        )
        .unwrap();
        assert_eq!(pool.health, "DEGRADED");
        assert_eq!(pool.errors, 7);
        // The faulted leaf is reported under its configured disk label.
        assert_eq!(
            pool.bad_devices,
            [BadDevice {
                name: "H2".into(),
                state: "FAULTED".into()
            }]
        );
        let scan = pool.scan.unwrap();
        assert_eq!(scan.kind, ScanKind::Resilver);
        assert!(scan.paused);
        assert_eq!(scan.eta_secs, None);
        assert!(scan.percent > 99.0);
    }

    #[test]
    fn missing_pool_is_an_error() {
        assert!(parse_pool("other", FINISHED, LIST, 0, &|_| None).is_err());
        assert!(parse_pool("storage01", "not json", LIST, 0, &|_| None).is_err());
    }
}
