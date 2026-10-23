# Security Policy

## Reporting a vulnerability

Email: `nietonchique@users.noreply.github.com` (via
[GitHub's private vulnerability reporting](https://github.com/nietonchique/aoostar-lcd/security/advisories/new)
if available, or plain email otherwise).

Please include:
- description of the issue
- reproduction steps or proof of concept
- affected versions
- potential impact

## Scope

The panel runs as root (it needs raw device access: serial port, Docker
socket, smartctl, zpool, nvidia-smi, cgroup filesystem). Security issues in
these interfaces are in scope. Issues in downstream dependencies should be
reported to the respective maintainers.

## Hardening

The systemd unit ships with `ProtectSystem=strict`, `ProtectHome`,
`ProtectKernel*`, `NoNewPrivileges`, `PrivateTmp`, `RestrictSUIDSGID`,
`LockPersonality`, `MemoryMax=128M`, `TasksMax=64`. The panel makes no
outbound connections except to the optional Grafana and Prometheus endpoints
configured by the user.
