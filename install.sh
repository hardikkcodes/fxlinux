#!/usr/bin/env bash
# FxLinux installer: system packages -> Rust toolchain -> build -> wire into
# the system (filter-chain config, systemd service, autostart entry).
# Safe to re-run: apt/rustup are idempotent, and `fxlinux-app install` skips
# the filter-chain config if it already exists.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "==> FxLinux installer"

if ! command -v apt-get &>/dev/null; then
    echo "error: this installer only supports apt-based distros (Ubuntu/Debian)." >&2
    exit 1
fi

PACKAGES=(
    build-essential pkg-config curl clang libclang-dev
    libgtk-4-dev libadwaita-1-dev
    libpipewire-0.3-dev libspa-0.2-dev libspa-0.2-modules-extra
    lsp-plugins-lv2 calf-plugins lilv-utils
)

echo "==> Installing system packages (will prompt for your sudo password)"
sudo apt-get update
sudo apt-get install -y "${PACKAGES[@]}"

if ! command -v cargo &>/dev/null && [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

if ! command -v cargo &>/dev/null; then
    echo "==> Installing Rust (rustup, stable toolchain)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

echo "==> Building FxLinux (release)"
cargo build --release

echo "==> Wiring FxLinux into the system"
./target/release/fxlinux-app install

cat <<EOF

==> Done.

Set "FxLinux" as your default audio output (desktop Sound settings, or
\`wpctl set-default <id>\`), then launch the control panel with:

    $SCRIPT_DIR/target/release/fxlinux-app

It will also start automatically on your next login.
EOF
