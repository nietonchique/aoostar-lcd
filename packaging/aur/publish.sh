#!/usr/bin/env bash
# Publish the AUR package once an AUR account with an uploaded SSH key exists.
# Usage: packaging/aur/publish.sh <ssh-key-path> [pkgname]
# Defaults: key ~/.ssh/aur, package aoostar-lcd-bin.
set -euo pipefail

readonly KEY="${1:?usage: publish.sh <ssh-key-path> [pkgname]}"
readonly PKG="${2:-aoostar-lcd-bin}"
readonly AUR_URL="ssh://aur@aur.archlinux.org/${PKG}.git"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
KEY_ABS="$(cd -- "$(dirname -- "${KEY}")" && pwd)/$(basename -- "${KEY}")"
readonly KEY_ABS

for command in git makepkg ssh; do
  command -v "${command}" >/dev/null 2>&1 || {
    printf 'missing command: %s\n' "${command}" >&2
    exit 69
  }
done
[[ -f "${KEY_ABS}" ]] || { printf 'key not found: %s\n' "${KEY_ABS}" >&2; exit 66; }

GIT_SSH_COMMAND="$(printf 'ssh -i %q -o IdentitiesOnly=yes' "${KEY_ABS}")"
export GIT_SSH_COMMAND

work_dir="$(mktemp -d)"
trap 'rm -rf -- "${work_dir}"' EXIT
readonly work_dir

# The package does not exist yet: clone fails, start from an empty repo.
if ! git clone -q "${AUR_URL}" "${work_dir}/${PKG}"; then
  printf 'clone failed - account ready and key uploaded? see packaging/README.md\n' >&2
  exit 1
fi

install -m0644 "${SCRIPT_DIR}/PKGBUILD" "${work_dir}/${PKG}/PKGBUILD"
install -m0644 "${SCRIPT_DIR}/.SRCINFO" "${work_dir}/${PKG}/.SRCINFO"
(
  cd -- "${work_dir}/${PKG}"
  makepkg --printsrcinfo > .SRCINFO
  git add PKGBUILD .SRCINFO
  git commit -q -m "aoostar-lcd-bin $(makepkg --printsrcinfo | awk -F' = ' '/^pkgver/{print $2; exit}')"
  git push origin master
)
printf 'published %s\n' "${PKG}"
