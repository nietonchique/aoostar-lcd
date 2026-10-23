# Where the data comes from

Everything the panel shows is read-only local state plus two optional
scrapers. Nothing is written to the panel but pixels; nothing leaves the
host.

| Source | Reads | Every | Feeds |
|---|---|---|---|
| `/proc` | `stat`, `loadavg`, `meminfo`, `uptime`, `diskstats`, `mountinfo` | 1 s | CPU load, RAM/swap, per-disk read/write/IOPS/busy, volume usage |
| `/sys/class/hwmon` | temperature and fan sensors | 1 s | CPU/GPU/DIMM temps, fan rpm |
| cgroup v2 tree | per-cgroup CPU and memory | 2 s | "top workloads" page (containers, services) |
| `/proc/spl/kstat/zfs/<pool>/objset-*` | per-dataset logical I/O counters | 1 s | busiest datasets — who makes the HDDs work |
| Docker Engine API | `/containers/json` over `services.docker_socket`; `CI_*` env of Woodpecker step containers | 5 s | unhealthy/restarting containers, running CI pipelines |
| `systemctl` | `list-units` + `show` for configured stacks/units; `list-jobs` | 5 s / on stop | service outages, failed units, maintenance flags, shutdown-vs-reboot |
| `nvidia-smi` | utilization, temperature, power, VRAM | 5 s | GPU tile |
| `zpool status -j`, `zpool list` | pool/leaf health, capacity, scan | 30 s | ZFS page, pool alarms |
| `smartctl -j -H -c -A` | health verdict, temperature, self-test progress | 60 s | HDD tiles, disk alarms |
| Grafana alert API *(optional)* | firing alerts over HTTP with a Viewer token | 15 s | extra problems; `outage_families` escalate to ALARM |
| Prometheus *(optional)* | Woodpecker queue gauges | 30 s | CI queue length and wait |

## How merging works

- `/proc`-level sources are sampled on the render thread every second; each
  slow source refreshes on its own background-thread interval and the merge
  reads only the latest stored value, so a 30 s source never delays a frame.
- A failed source keeps its last good value for three intervals, then the
  page shows `--`. An unreachable Grafana becomes a problem row itself.
- Docker container environments are not read: only the `CI_*` keys of
  Woodpecker step containers are kept, everything else is dropped.
- Disk identities come from the configuration (`[[disk]].by_id`); ZFS leaf
  devices and `smartctl` results are mapped back to those labels for display.
