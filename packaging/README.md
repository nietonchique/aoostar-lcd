# Packaging sources

Everything needed to package `aoostar-lcd` for Linux distributions. The
tag-driven `.github/workflows/release.yml` builds the x86_64 tarball, `.deb`
(cargo-deb) and `.rpm` (cargo-generate-rpm) and attaches them with a
`SHA256SUMS.txt` to the GitHub release; the metadata lives in
`crates/aoostar-lcd/Cargo.toml` under `[package.metadata.deb]` and
`[package.metadata.generate-rpm]`.

- `aur/PKGBUILD` — [AUR](https://aur.archlinux.org) package `aoostar-lcd-bin`
  from the release tarball. `sha256sums` is filled from the release's
  `SHA256SUMS.txt` (or `updpkgsums`); regenerate `.SRCINFO` with
  `makepkg --printsrcinfo > .SRCINFO`.
- `aur/PKGBUILD-git` — source build tracking `main`.
- `systemd/aoostar-lcd.service` — the packaged unit; an exact copy of
  `dist/aoostar-lcd.service` with `/usr/local/bin` replaced by `/usr/bin`.
  The release workflow fails on drift.

## Maintainer: publishing to the AUR

```bash
# one-time: register an AUR account and upload your SSH public key
git clone ssh://aur@aur.archlinux.org/aoostar-lcd-bin.git
cd aoostar-lcd-bin
cp /path/to/repo/packaging/aur/PKGBUILD .
updpkgsums                # or paste the sha256 from SHA256SUMS.txt
makepkg --printsrcinfo > .SRCINFO
makepkg -si               # local install check
namcap PKGBUILD           # lint (pacman -S namcap)
git add PKGBUILD .SRCINFO && git commit -m "v1.0.1" && git push
```

Debian/Ubuntu and Fedora/RHEL users install the `.deb`/`.rpm` from the
[releases page](https://github.com/nietonchique/aoostar-lcd/releases) — the
config lands at `/etc/aoostar-lcd.toml` (a conffile, local edits survive
upgrades) and the unit at `/usr/lib/systemd/system/aoostar-lcd.service`.
