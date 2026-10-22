# aoostar-lcd

Glanceable front-panel status display for the AOOSTAR WTR MAX home server
(960×376 USB-serial LCD, ~4.3"). Built to be read from a distance in the
moments the owner actually looks at it:

- a disk fails or a critical service is down without a maintenance flag →
  the whole screen blinks red with **ALARM** and every broken thing;
- the server is booting → a **STARTUP** page tracks the watched services
  coming up instead of a false alarm about everything being down;
- the server is stopping → **SHUTDOWN / REBOOT** shows what has stopped and
  what is still going down;
- something else is wrong → the status bar turns amber and names it;
- the HDDs are noisy → per-disk read/write, scrub/SMART-test progress,
  busiest datasets;
- CI takes long → which pipelines run, for how long, how many wait;
- otherwise: temperatures, load, free memory and free space.

UI language: English or Russian, set by `[display] language` in the config.

## What is on the screen

**Alarm takeover.** Any of these replaces every page with a full-screen alarm
that flips red/black on every frame (~1.3 s, the panel's full-repaint time):
a configured disk is missing or reports SMART `FAILED`, a pool leaf device is
not `ONLINE` (shown by disk label), a pool is not `ONLINE`, or a critical
service outage without a maintenance flag. One problem is shown huge; several
are grouped per object and laid out in 1–3 columns with one common font size;
beyond 12 lines the last reads "and N more". Failed timer jobs, CI step
containers and warning-level outages never raise the alarm.

Every other page shares a 64 px **status bar**: state on the left
(`ALL RUNNING` green / `PROBLEM: …` amber / `MAINTENANCE: …` grey), then
chips (`alerts N`, `CI 2 · 17 min`, `HDD 71%`), page dots and the clock.

| Page | Shown | Content |
|---|---|---|
| Startup | fresh boot while watched services are still down | big `up / total`, stopwatch since boot, one row per service not up yet |
| Home | always, double dwell | CPU temp + load, free RAM, GPU temp + util/VRAM, HDD busy % (+ read/write when busy) |
| Problems | while anything is firing | outages first, then Grafana alerts and failed jobs grouped by service, with age and count |
| CI | while pipelines run or wait | repo, pipeline number, step, branch, stopwatch; queue length and wait |
| HDD | HDD busy ≥ 10 %, running scrub/resilver or SMART self-test | one row per HDD (any count): busy %, read MB/s, write MB/s, temperature or self-test |
| ZFS | together with HDD | every configured pool: health, free space, scan; busiest datasets with read/write |
| Temps | always | CPU, GPU, DIMM, fan rpm, every NVMe and HDD (4 × up to 3 tiles) |
| Load | always | top CPU and memory workloads (containers, CI pipelines, services) |
| Space | always | one row per volume and pool: free, total, meter, used % |

Headline values are 84–108 px, detail text 38–42 px, nothing below 34 px.
Colour encodes state only (thresholds in the configuration).

## Boot and power transitions

The systemd unit is ordered after local file systems and the panel's own
serial device (not after Docker), so boot tracking starts as early as the
hardware allows. Until every watched stack/unit is active — or
`services.startup_window_secs` (600 s) pass since boot — the startup page
replaces the rotation, and critical service outages do not raise the alarm
while it is up; hardware alarms (missing disks, degraded pools) show in every
phase.

When systemd stops the unit on poweroff or reboot, `ExecStopPost` runs
`aoostar-lcd shutdown`: it reads `systemctl list-jobs` to tell SHUTDOWN from
REBOOT, then shows the watched services stopping — `stopped 5 / 13`, the
names still up, the container count — once per second for up to 80 s (inside
the 90 s stop budget). Nothing is ordered after the unit, so this parallel
tracking never delays the poweroff: the loop exits as soon as every watched
service is down. With no shutdown job pending (a manual stop, a deploy
restart) it switches the panel off immediately. A poweroff ends with the
panel dark; after a reboot the last frame stays on the panel until the
startup page replaces it.

## Data sources (all read-only)

| Source | Interval | Used for |
|---|---|---|
| `/proc`, `/sys` (stat, meminfo, diskstats, hwmon, cgroup v2) | 1 s | CPU, memory, temps, fans, disk rates, workloads |
| `/proc/spl/kstat/zfs/<pool>/objset-*` | 1 s | per-dataset logical I/O ("who makes the HDDs work") |
| Docker Engine API (`/run/docker.sock`) | 5 s | unhealthy/restarting containers, running Woodpecker CI steps (`CI_*` env only) |
| `systemctl` | 5 s | inactive configured stacks/units, failed units, maintenance flags |
| `systemctl list-jobs` | on stop | shutdown vs reboot detection |
| `nvidia-smi` | 5 s | GPU |
| Grafana alert API (Viewer token) | 15 s | firing alerts; `alert_family` decides outage vs alert |
| Prometheus | 30 s | Woodpecker queue length and oldest wait |
| `zpool status -j` / `zpool list` | 30 s | health, errors, non-`ONLINE` leaf devices (mapped to disk labels), scrub/resilver progress |
| `smartctl -j -H -c -A` | 60 s | HDD temperature, SMART health verdict, self-test progress |

A source that fails keeps its last value briefly and then shows `--`; an
unreachable Grafana is itself reported as a problem.

## Install

Requirements: Linux with systemd, a Rust toolchain (1.88+), `libudev-dev` and
`pkg-config`.

```bash
git clone https://github.com/nietonchique/aoostar-lcd
cd aoostar-lcd
cargo build --locked --release -p aoostar-lcd
sudo install -Dm0755 target/release/aoostar-lcd /usr/local/bin/aoostar-lcd
sudo install -Dm0644 config.example.toml /etc/aoostar-lcd.toml  # then edit it
sudo install -Dm0644 dist/aoostar-lcd.service /etc/systemd/system/aoostar-lcd.service
sudo systemctl daemon-reload && sudo systemctl enable --now aoostar-lcd
```

Validate a configuration without touching the panel:

```bash
aoostar-lcd check --config /etc/aoostar-lcd.toml
```

## Configuration

`config.example.toml` documents every key. Sections: `[display]` (device,
language, page seconds, heartbeat, clock offset), `[thresholds]` (warning /
critical boundaries per metric), `[[disk]]` / `[[volume]]` (what to watch,
by-id identities only for disks), `[zfs]` (pools), `[services]` (systemd
templates and units that must be active, boot window, maintenance dir,
Docker socket), `[alerts]` (optional Grafana), `[ci]` (optional Prometheus).

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Offline preview of every page from a snapshot fixture
cargo run -p aoostar-lcd -- preview --config config.example.toml \
  --fixture crates/aoostar-lcd/fixtures/boot.json --out-dir /tmp/lcd

# Preview the live state of the host
sudo aoostar-lcd snapshot > /tmp/live.json
cargo run -p aoostar-lcd -- preview --config /etc/aoostar-lcd.toml \
  --fixture /tmp/live.json --out-dir /tmp/lcd
```

CLI: `run` (drive the panel), `check` (validate config + one collection),
`snapshot` (print a snapshot as JSON), `preview` (render pages to PNG),
`shutdown` (shutdown/reboot progress; used by `ExecStopPost`), `off`.

## Layout

```text
crates/asterctl-lcd/   serial protocol (from zehnm/aoostar-rs, MIT/Apache-2.0)
crates/aoostar-lcd/    collector, renderer, rotation, i18n, CLI
  fixtures/            snapshot fixtures for offline previews
  fonts/               Roboto Condensed Medium/Bold (OFL), compiled in
config.example.toml    documented example configuration
dist/                  example systemd unit
```

## License

The protocol crate keeps the upstream MIT OR Apache-2.0 license
([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)); everything
else is under the same dual license. Roboto Condensed is under the SIL Open
Font License ([crates/aoostar-lcd/fonts/OFL.txt](crates/aoostar-lcd/fonts/OFL.txt)).
Serial protocol and panel handling credit [zehnm/aoostar-rs](https://github.com/zehnm/aoostar-rs).

See [README.ru.md](README.ru.md) for the Russian version.
