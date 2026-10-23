# aoostar-lcd

Glanceable status panel for the AOOSTAR WTR MAX front LCD (960×376, USB
serial). One systemd service draws every page: alarms, boot and shutdown
progress, disks, ZFS, Docker, CI. UI: English or Russian.

| | | | |
|---|---|---|---|
| ![home](docs/img/home.png) | ![startup](docs/img/startup.png) | ![alarm](docs/img/alarm.png) | ![shutdown](docs/img/shutdown.png) |
| ![problems](docs/img/problems.png) | ![ci](docs/img/ci.png) | ![hdd](docs/img/hdd.png) | ![zfs](docs/img/zfs.png) |

Pages rotate by relevance: the summary always shows; problems, CI and HDD
pages join only while relevant; a full-screen blinking **ALARM** takes over
when a configured disk/pool breaks or a critical service is down. On boot a
**STARTUP** page lists watched services that are not up yet (and suppresses
service alarms while it shows); on poweroff/reboot an `ExecStopPost` hook
draws **SHUTDOWN / REBOOT** progress until every watched service is down.
Russian UI (screenshots above) — `[display] language = "ru"`.

## Where the data comes from

Everything is read-only local state plus two optional scrapers. Nothing is
written to the panel but pixels; nothing leaves the host.

| Source | Reads | Every | Feeds |
|---|---|---|---|
| `/proc` | `stat`, `loadavg`, `meminfo`, `uptime`, `diskstats`, `mountinfo` | 1 s | CPU load, RAM/swap, per-disk read/write/IOPS/busy, volume usage |
| `/sys/class/hwmon` | temperature and fan sensors | 1 s | CPU/GPU/DIMM temps, fan rpm |
| cgroup v2 tree | per-cgroup CPU and memory | 2 s | "top workloads" page (containers, services) |
| `/proc/spl/kstat/zfs/<pool>/objset-*` | per-dataset logical I/O counters | 1 s | busiest datasets — who makes the HDDs work |
| Docker Engine API | `/containers/json` over `/run/docker.sock`; `CI_*` env of Woodpecker step containers | 5 s | unhealthy/restarting containers, running CI pipelines |
| `systemctl` | `list-units` + `show` for configured stacks/units; `list-jobs` | 5 s / on stop | service outages, failed units, maintenance flags, shutdown-vs-reboot |
| `nvidia-smi` | utilization, temperature, power, VRAM | 5 s | GPU tile |
| `zpool status -j`, `zpool list` | pool/leaf health, capacity, scan | 30 s | ZFS page, pool alarms |
| `smartctl -j -H -c -A` | health verdict, temperature, self-test progress | 60 s | HDD tiles, disk alarms |
| Grafana alert API *(optional)* | firing alerts over HTTP with a Viewer token | 15 s | extra problems; `outage_families` escalate to ALARM |
| Prometheus *(optional)* | Woodpecker queue gauges | 30 s | CI queue length and wait |

A failed source keeps its last value for three intervals, then shows `--`;
an unreachable Grafana becomes a problem row itself.

## Configuration

One TOML file (`/etc/aoostar-lcd.toml`); [`config.example.toml`](config.example.toml)
documents every key.

| Section | Keys | Meaning |
|---|---|---|
| `[display]` | `device`, `language` (`en`/`ru`), `page_seconds`, `heartbeat`, `clock_utc_offset_minutes` | panel device, UI language, rotation speed, clock shift |
| `[thresholds]` | `*_temp_c`, `memory_used_percent`, `space_used_percent`, `hdd_busy_percent`, `ci_minutes` — each `[warning, critical]` | when values turn amber/red and which pages join the rotation |
| `[[disk]]` | `label` (1–4 chars), `kind` (`nvme`/`hdd`), `by_id` | disks to watch; **by-id paths only**; missing or SMART-failed → ALARM |
| `[[volume]]` | `label`, `path` | filesystems on the Space page |
| `[zfs]` | `pools` (first = primary) | pools to watch; leaf not `ONLINE` → ALARM |
| `[services]` | `templates`, `units`, `startup_window_secs`, `maintenance_dir`, `docker_socket` | systemd units that must be active; `<dir>/<unit>` or `<dir>/ALL` marks intentional stops; boot-window length |
| `[alerts]` *(optional)* | `grafana_url`, `grafana_host`, `token_file`, `local_node`, `outage_families` | Grafana alert integration |
| `[ci]` *(optional)* | `prometheus_url` | Woodpecker queue metrics |

Boot and shutdown behaviour: the unit is ordered only after local file
systems and the panel's serial device — never after Docker — so the STARTUP
page appears as early as the hardware allows, and at shutdown the service
stops in the first wave, keeping the progress page up for up to 80 s (inside
systemd's 90 s stop budget) while the stacks go down in parallel. With no
shutdown in progress the hook just switches the panel off; a poweroff ends
dark, a reboot keeps the last frame until STARTUP replaces it.

## Install

Linux + systemd, Rust 1.88+, `libudev-dev`, `pkg-config`.

```bash
cargo build --locked --release -p aoostar-lcd
sudo install -Dm0755 target/release/aoostar-lcd /usr/local/bin/aoostar-lcd
sudo install -Dm0644 config.example.toml /etc/aoostar-lcd.toml   # edit it
sudo install -Dm0644 dist/aoostar-lcd.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now aoostar-lcd
```

CLI: `run`, `check` (validate config, no panel writes), `snapshot` (JSON),
`preview` (render pages to PNG from a snapshot), `shutdown`, `off`.

## License

MIT OR Apache-2.0; Roboto Condensed under SIL OFL. Serial protocol from
[zehnm/aoostar-rs](https://github.com/zehnm/aoostar-rs).
[Русская версия](README.ru.md).
