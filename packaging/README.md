# Packaging sources and the release runbook

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

## Maintainer: cutting a release

The private Gitea repository is the source of truth; GitHub is published
from it and every push to its `main` auto-deploys to the home server.

1. Bump `version` in `crates/aoostar-lcd/Cargo.toml` (the release workflow
   refuses a tag that does not match it).
2. Commit, push to Gitea (`origin main`) and publish the public subset:
   `publish/publish.sh "<message>"`.
3. Tag: `git tag -a vX.Y.Z gh-main -m "aoostar-lcd vX.Y.Z" && git push github vX.Y.Z`.
4. The GitHub release workflow builds the tarball, `.deb`, `.rpm` and
   `SHA256SUMS.txt`, and attaches them to the release. No secrets needed.
5. Update the AUR package:
   `bash packaging/aur/publish.sh ~/.ssh/id_ed25519` — it bumps `pkgver`,
   refreshes the checksum from the release, regenerates `.SRCINFO` and pushes.
6. One-command installs (see `packaging/install.sh`) pick the new release up
   immediately; no further steps.

## One-command install

```sh
curl -fsSL https://raw.githubusercontent.com/nietonchique/aoostar-lcd/main/packaging/install.sh | sudo sh
```

Detects apt/dnf/pacman and installs the matching package (AUR via yay);
falls back to the release tarball on anything else. An existing
`/etc/aoostar-lcd.toml` is never overwritten.
