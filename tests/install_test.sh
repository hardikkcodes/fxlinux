#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck disable=SC1091
. "$ROOT_DIR/install.sh"

# Detection only checks for these commands. The tests never invoke them.
# shellcheck disable=SC2032
apt-get() { :; }
dpkg-query() { :; }
# shellcheck disable=SC2032
pacman() { :; }

TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fxlinux-install-test.XXXXXX")"
trap 'rm -rf -- "$TEST_DIR"' EXIT

assert_contains() {
    local expected="$1"
    shift
    local item

    for item in "$@"; do
        [[ "$item" == "$expected" ]] && return
    done
    echo "expected package '$expected' was not found" >&2
    exit 1
}

printf 'ID=ubuntu\n' >"$TEST_DIR/ubuntu"
detect_platform "$TEST_DIR/ubuntu"
[[ "$PACKAGE_MANAGER" == "apt" ]]
assert_contains build-essential "${PACKAGES[@]}"
assert_contains libpipewire-0.3-dev "${PACKAGES[@]}"

printf 'ID=cachyos\n' >"$TEST_DIR/cachyos"
detect_platform "$TEST_DIR/cachyos"
[[ "$PACKAGE_MANAGER" == "pacman" ]]
assert_contains base-devel "${PACKAGES[@]}"
assert_contains libpipewire "${PACKAGES[@]}"
assert_contains pipewire-audio "${PACKAGES[@]}"
assert_contains lsp-plugins-lv2 "${PACKAGES[@]}"

printf 'ID=fedora\n' >"$TEST_DIR/unsupported"
if (detect_platform "$TEST_DIR/unsupported") 2>/dev/null; then
    echo "unsupported distro detection unexpectedly succeeded" >&2
    exit 1
fi

echo "install tests: PASS"
