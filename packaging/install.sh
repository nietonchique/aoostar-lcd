#!/bin/sh
# One-command installer for aoostar-lcd: downloads the latest release
# package for this distro and installs it. Run as root or with sudo:
#
#   curl -fsSL https://raw.githubusercontent.com/nietonchique/aoostar-lcd/main/packaging/install.sh | sudo sh
#
# Installs /usr/bin/aoostar-lcd, the systemd unit and a default
# /etc/aoostar-lcd.toml (never overwrites an existing config).
set -eu

REPO="nietonchique/aoostar-lcd"
ARCH_MAP='{
  "x86_64":  "x86_64-unknown-linux-gnu",
  "aarch64": "aarch64-unknown-linux-gnu"
}'

log() { printf 'install: %s\n' "$1"; }
die() { printf 'install: %s\n' "$1" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run as root (sudo sh)"

UNAME_M="$(uname -m)"
TRIPLE="$(printf '%s' "$ARCH_MAP" | sed -n "s/.*\"$UNAME_M\": *\"\([^\"]*\)\".*/\1/p")"
[ -n "$TRIPLE" ] || die "unsupported architecture $UNAME_M"

# Latest version tag (v-prefixed, newest first).
VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/tags" |
  sed -n 's/.*"name": *"\(v[0-9][^"]*\)".*/\1/p' | head -n 1)"
[ -n "$VERSION" ] || die "cannot find the latest release tag"
log "installing $VERSION"

BASE="https://github.com/$REPO/releases/download/$VERSION"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if command -v apt-get >/dev/null 2>&1; then
  ASSET="aoostar-lcd_${VERSION#v}-1_amd64.deb"
  [ "$TRIPLE" = "x86_64-unknown-linux-gnu" ] || die "no .deb for $TRIPLE yet"
  curl -fsSL -o "$TMP/pkg" "$BASE/$ASSET"
  apt-get install -y "$TMP/pkg"
elif command -v dnf >/dev/null 2>&1; then
  ASSET="aoostar-lcd-${VERSION#v}-1.$UNAME_M.rpm"
  curl -fsSL -o "$TMP/pkg" "$BASE/$ASSET"
  dnf install -y "$TMP/pkg"
elif command -v pacman >/dev/null 2>&1; then
  if command -v yay >/dev/null 2>&1; then
    yay -S --needed aoostar-lcd-bin
  else
    die "install yay first, or use the tarball: $BASE/aoostar-lcd-$VERSION-$TRIPLE.tar.gz"
  fi
else
  ASSET="aoostar-lcd-$VERSION-$TRIPLE.tar.gz"
  curl -fsSL -o "$TMP/pkg.tgz" "$BASE/$ASSET"
  tar -C "$TMP" -xzf "$TMP/pkg.tgz"
  cd "$TMP"/aoostar-lcd-*/
  install -Dm755 aoostar-lcd /usr/bin/aoostar-lcd
  [ -f /etc/aoostar-lcd.toml ] || install -Dm644 config.example.toml /etc/aoostar-lcd.toml
  install -Dm644 aoostar-lcd.service /usr/lib/systemd/system/aoostar-lcd.service
  systemctl daemon-reload
fi

log "installed; edit /etc/aoostar-lcd.toml (disks, services), then:"
log "  systemctl enable --now aoostar-lcd"
