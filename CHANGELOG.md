# Changelog

## v1.1.0 — 2026-10-23

### Added
- 8 new languages: Ukrainian, Polish, German, French, Spanish, Turkish,
  Chinese (Simplified), Korean — 10 total
- `[display] language = "auto"` follows the system locale, falls back to English
- `[display] i18n_dir` loads extra/override translation tables at runtime
- CJK rendering via subset Noto Sans SC/KR fonts (per-character fallback)
- Pixel-level layout regression tests (render pages, measure PNG ink symmetry)
- One-command installer (`packaging/install.sh`)
- Release runbook (`packaging/README.md`)
- AUR package `aoostar-lcd-bin`
- `.deb` (cargo-deb) and `.rpm` (cargo-generate-rpm) release artifacts
- Rotation GIF demo in README

### Changed
- UI strings moved from hardcoded Rust to `i18n/*.toml` tables
- Row components center by real ink extents (descender-aware)
- Home page meter keeps uniform panel padding
- Problems page rows are pixel-symmetric (top/bottom padding equal)
- English-only documentation (Russian docs removed)
- README rewritten for scannability

### Fixed
- Alarm screen vertically centered as one block
- Problems overflow row no longer overlaps two lines of text
- udev race on boot: unit ordered after the panel's `.device` unit
- Woodpecker redirects table missing id sequence (after repo rename)

## v1.0.1 — 2026-10-23

### Fixed
- Release workflow no longer stages build artifacts in the repo `dist/` directory

## v1.0.0 — 2026-10-23

Initial public release.
