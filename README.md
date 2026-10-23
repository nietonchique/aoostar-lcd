# aoostar-lcd

Glanceable status panel for the AOOSTAR WTR MAX front LCD (960×376, USB
serial). One systemd service draws every page: alarms, boot and shutdown
progress, disks, ZFS, Docker, CI. UI: English or Russian — and any language
you drop in as a TOML table.

![rotation](docs/img/rotation.gif)

| | | | |
|---|---|---|---|
| ![home](docs/img/home.png) | ![startup](docs/img/startup.png) | ![alarm](docs/img/alarm.png) | ![reboot](docs/img/reboot.png) |
| ![problems](docs/img/problems.png) | ![ci](docs/img/ci.png) | ![hdd](docs/img/hdd.png) | ![zfs](docs/img/zfs.png) |

Pages rotate by relevance: the summary always shows; problems, CI and HDD
pages join only while relevant; a full-screen blinking **ALARM** takes over
when a configured disk/pool breaks or a critical service is down. On boot a
**STARTUP** page lists watched services that are not up yet; on poweroff or
reboot an `ExecStopPost` hook draws **SHUTDOWN / REBOOT** progress until
every watched service is down.


- **[Where the data comes from](docs/data-sources.md)** — 11 read-only
  sources, what each reads, at what interval, and how failures degrade.
- **[Configuration](docs/configuration.md)** — every section and key.
- **[Boot and power transitions](docs/boot-shutdown.md)** — why the unit is
  wired the way it is.

## Languages

`i18n/en.toml` and `i18n/ru.toml` are embedded in the binary; set
`[display] language`. A new language is one translated TOML file — copy
`i18n/en.toml`, translate the values, name it with your language code and
`[display] i18n_dir` can load tables without rebuilding. Plural forms are
declared per language (`plural_family = "en" | "ru"`).

## Install

Linux + systemd, Rust 1.88+, `libudev-dev`, `pkg-config`.

```bash
cargo build --locked --release -p aoostar-lcd
sudo install -Dm0755 target/release/aoostar-lcd /usr/local/bin/aoostar-lcd
sudo install -Dm0644 config.example.toml /etc/aoostar-lcd.toml   # edit it
sudo install -Dm0644 dist/aoostar-lcd.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now aoostar-lcd
```

Or straight from a release (apt/dnf/pacman/tarball autodetected):

```sh
curl -fsSL https://raw.githubusercontent.com/nietonchique/aoostar-lcd/main/packaging/install.sh | sudo sh
```

CLI: `run`, `check` (validate config, no panel writes), `snapshot` (JSON),
`preview` (render pages to PNG from a snapshot), `shutdown`, `off`.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Offline preview of every page from a snapshot fixture
cargo run -p aoostar-lcd -- preview --config config.example.toml \
  --fixture crates/aoostar-lcd/fixtures/busy.json --out-dir /tmp/lcd
```

## License

MIT OR Apache-2.0; Roboto Condensed under SIL OFL. Serial protocol from
[zehnm/aoostar-rs](https://github.com/zehnm/aoostar-rs).
