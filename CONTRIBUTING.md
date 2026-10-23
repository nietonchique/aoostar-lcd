# Contributing

Thanks for considering a contribution!

## Adding a translation

The panel speaks TOML. Copy `i18n/en.toml`, translate the values, name the
file with your language code:

```bash
cp i18n/en.toml i18n/xx.toml   # xx = your ISO 639-1 code
```

Key points:

- `plural_family` picks the plural rule: `"en"` (one/other), `"ru"` (Slavic
  one/few/many), `"none"` (CJK — every count uses "other"). If your language
  needs a different rule, open an issue and we'll add it.
- `[details]` and `[subjects]` are optional: collector messages pass through
  in English when a key is missing.
- Missing UI keys fall back to English — a partial translation is still
  useful.
- CJK text renders via subset Noto Sans SC/KR fonts; new codepoints outside
  the existing subsets will show as tofu until the fonts are regenerated
  (`fonttools varLib.instancer` + `subset`, see `crates/aoostar-lcd/fonts/`).

Test it:

```bash
cargo run -p aoostar-lcd -- preview \
  --config config.example.toml \
  --fixture crates/aoostar-lcd/fixtures/busy.json \
  --out-dir /tmp/lcd
```

## Development workflow

```bash
cargo fmt --all --check         # formatting
cargo clippy --workspace --all-targets -- -D warnings  # linting
cargo test --workspace          # 92+ tests including pixel-level layout checks
```

All three run in CI on every push and PR.

## Reporting bugs

Open an issue with:
- what you saw (screenshot if it's a rendering bug)
- what you expected
- your config (redact serial numbers / IPs)
- `aoostar-lcd check --config /etc/aoostar-lcd.toml` output
