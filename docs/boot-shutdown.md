# Boot, shutdown and reboot

How the panel behaves across power transitions, and why the systemd unit is
wired the way it is.

## Boot — the STARTUP page

The unit is ordered after local file systems and the panel's own serial
device — deliberately **not** after `docker.service` — so it starts as early
as the hardware allows, before Docker and the stacks come up.

Until every watched service (`[services]` templates + units) is active, or
`startup_window_secs` (default 600 s) pass since boot, the STARTUP page
replaces the whole rotation: big `up / total`, a stopwatch since boot and one
row per service that is not up yet. Critical service outages do not raise
the alarm while this page shows — services being down is what booting looks
like. Hardware alarms (missing disks, degraded pools) show in every phase.

## Shutdown and reboot — the SHUTDOWN / REBOOT page

The unit's `ExecStopPost` runs `aoostar-lcd shutdown`, which:

1. reads `systemctl list-jobs` to tell ВЫКЛЮЧЕНИЕ (poweroff/halt) from
   ПЕРЕЗАГРУЗКА (reboot);
2. draws the watched services stopping — `stopped 5 / 13`, the names still
   up in amber, failed units in red, the container count — one frame per
   second;
3. exits as soon as every watched service is down, or after 80 s
   (`--max-secs`, inside systemd's 90 s stop budget).

Nothing is ordered after the unit, so this parallel tracking never delays
the poweroff: it stops in the first wave on purpose, in parallel with the
stacks it watches.

| Situation | Behaviour |
|---|---|
| `systemctl poweroff` / halt | SHUTDOWN page, panel switches off at the end |
| `systemctl reboot` | REBOOT page; the last frame stays on the panel until the startup page replaces it after boot |
| `systemctl stop aoostar-lcd` (no shutdown in progress) | panel switches off immediately |
| deploy restart (stop + start) | same as a manual stop — no shutdown page |

## Unit wiring

`dist/aoostar-lcd.service` is the reference unit:

```ini
After=local-fs.target dev-serial-by\x2did-usb\x2dSynwit_USB_Virtual_COM\x2dif00.device
Wants=dev-serial-by\x2did-usb\x2dSynwit_USB_Virtual_COM\x2dif00.device
ConditionPathExists=/dev/serial/by-id/usb-Synwit_USB_Virtual_COM-if00
ExecStart=/usr/local/bin/aoostar-lcd run --config /etc/aoostar-lcd.toml
ExecStopPost=-/usr/local/bin/aoostar-lcd shutdown --config /etc/aoostar-lcd.toml
```

The `Wants=` + `After=` pair on the generated `.device` unit removes the udev
race: the service starts exactly when the panel exists, and a missing panel
skips the start (the condition) instead of failing the boot. Hardening flags
(`ProtectSystem=strict` and friends) apply to both `ExecStart` and
`ExecStopPost`.
