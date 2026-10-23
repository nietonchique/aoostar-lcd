# Configuration

One TOML file (`/etc/aoostar-lcd.toml`); every key is documented in the
shipped [`config.example.toml`](../config.example.toml).

| Section | Keys | Meaning |
|---|---|---|
| `[display]` | `device` | USB-serial device of the panel (by-id path) |
| | `language` | `en` or `ru` — panel UI language |
| | `page_seconds` | seconds each page stays on screen (Home: double) |
| | `heartbeat` | file touched after every frame (default `/run/aoostar-lcd/heartbeat`) |
| | `clock_utc_offset_minutes` | status-bar clock shift from UTC |
| `[thresholds]` | `cpu_temp_c`, `gpu_temp_c`, `nvme_temp_c`, `hdd_temp_c`, `ram_temp_c`, `memory_used_percent`, `space_used_percent`, `hdd_busy_percent`, `ci_minutes` — each `[warning, critical]` | values at or above a boundary take its colour; `hdd_busy_percent` warning also adds the HDD page to the rotation |
| `[[disk]]` | `label` (1–4 chars), `kind` (`nvme`/`hdd`), `by_id` | disks to watch, configuration order. by-id paths only — a missing disk or SMART `FAILED` raises the ALARM |
| `[[volume]]` | `label` (1–5 chars), `path` | filesystems shown on the Space page |
| `[zfs]` | `pools` | pools to watch; the first is primary (dataset names lose the prefix). A leaf device not `ONLINE` or a non-`ONLINE` pool raises the ALARM |
| `[services]` | `templates` | systemd template units whose loaded instances must be active (e.g. `homelab-compose@` for Docker Compose stacks) |
| | `units` | individual units that must be active |
| | `startup_window_secs` | boot window: how long the STARTUP page owns the screen (default 600) |
| | `maintenance_dir` | `<dir>/<name>` or `<dir>/ALL` marks an intentionally stopped unit |
| | `docker_socket` | Docker Engine socket path (default `/run/docker.sock`) |
| `[alerts]` *(optional)* | `grafana_url`, `grafana_host`, `token_file`, `local_node`, `outage_families` | Grafana alert API: firing alerts join the Problems page; families listed in `outage_families` escalate to ALARM |
| `[ci]` *(optional)* | `prometheus_url` | Prometheus endpoint for Woodpecker queue metrics |

## Semantics worth knowing

- **Maintenance vs outage.** A watched unit that is inactive while
  `<maintenance_dir>/<subject>` (or `<dir>/ALL`) exists is shown grey
  (maintenance), never as an outage. The directory lives on tmpfs, so a
  reboot clears it.
- **Problems merge by subject.** Several alerts for one service become one
  row with a count; outages sort above alerts, then severity, then age.
- **Colour is state, not decoration.** Every threshold is a `[warning,
  critical]` pair; nothing else on the panel uses colour.
- The disk `[[disk]]` list is authoritative for alarms: a disk that
  disappears or reports SMART `FAILED` raises the ALARM even during the boot
  window.
