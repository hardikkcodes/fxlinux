#!/usr/bin/env bash
# FxLinux installer: system packages -> Rust toolchain -> build -> wire into
# the system (filter-chain config, systemd service, autostart entry).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUSTUP_URL="https://sh.rustup.rs"
TEMP_FILES=()

cleanup() {
    if ((${#TEMP_FILES[@]})); then
        rm -f -- "${TEMP_FILES[@]}"
    fi
}

trap cleanup EXIT

die() {
    echo "error: $*" >&2
    exit 1
}

require_command() {
    local command_name="$1"
    local guidance="$2"

    command -v "$command_name" &>/dev/null || die "$command_name is required. $guidance"
}

detect_platform() {
    local os_release="${1:-/etc/os-release}"
    local distro_id

    [[ -r "$os_release" ]] || die "cannot read $os_release to identify the Linux distribution."

    # os-release is the distro's standard machine-readable identity file.
    # shellcheck disable=SC1090
    . "$os_release"
    distro_id="${ID:-}"

    case "$distro_id" in
        debian|ubuntu)
            command -v apt-get &>/dev/null || die "$distro_id requires apt-get, but apt-get was not found."
            command -v dpkg-query &>/dev/null || die "$distro_id requires dpkg-query, but dpkg-query was not found."
            PACKAGE_MANAGER="apt"
            PACKAGES=(
                build-essential pkg-config curl clang libclang-dev
                libgtk-4-dev libadwaita-1-dev
                libpipewire-0.3-dev libspa-0.2-dev libspa-0.2-modules-extra
                lsp-plugins-lv2 calf-plugins lilv-utils
            )
            ;;
        arch|cachyos)
            command -v pacman &>/dev/null || die "$distro_id requires pacman, but pacman was not found."
            PACKAGE_MANAGER="pacman"
            PACKAGES=(
                base-devel pkgconf curl clang gtk4 libadwaita
                libpipewire pipewire-audio
                calf lsp-plugins-lv2 lilv
            )
            ;;
        *)
            die "unsupported distribution '${distro_id:-unknown}'. Supported distributions are Debian, Ubuntu, Arch Linux, and CachyOS."
            ;;
    esac
}

collect_missing_packages() {
    local package
    MISSING_PACKAGES=()

    for package in "${PACKAGES[@]}"; do
        case "$PACKAGE_MANAGER" in
            apt)
                if ! dpkg-query -W -f='${db:Status-Abbrev}' "$package" 2>/dev/null | grep -q '^ii '; then
                    MISSING_PACKAGES+=("$package")
                fi
                ;;
            pacman)
                if ! pacman -Qq "$package" &>/dev/null; then
                    MISSING_PACKAGES+=("$package")
                fi
                ;;
        esac
    done
}

install_system_packages() {
    collect_missing_packages

    if ((${#MISSING_PACKAGES[@]} == 0)); then
        echo "==> System packages are already installed"
        return
    fi

    echo "==> Installing missing system packages (sudo may prompt for your password)"
    case "$PACKAGE_MANAGER" in
        apt)
            sudo apt-get update
            sudo apt-get install -y --no-install-recommends "${MISSING_PACKAGES[@]}"
            ;;
        pacman)
            sudo pacman -S --needed --noconfirm "${MISSING_PACKAGES[@]}"
            ;;
    esac
}

install_rust() {
    local rustup_installer
    local http_status
    local curl_status

    if command -v cargo &>/dev/null; then
        return
    fi

    if [[ -f "$HOME/.cargo/env" ]]; then
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
    fi

    if command -v cargo &>/dev/null; then
        return
    fi

    echo "==> Installing Rust (rustup, stable toolchain)"
    rustup_installer="$(mktemp "${TMPDIR:-/tmp}/fxlinux-rustup.XXXXXX")"
    TEMP_FILES+=("$rustup_installer")

    set +e
    http_status="$(curl --proto '=https' --tlsv1.2 --location --silent --show-error \
        --output "$rustup_installer" --write-out '%{http_code}' "$RUSTUP_URL")"
    curl_status=$?
    set -e

    if ((curl_status != 0)) || [[ ! "$http_status" =~ ^2[0-9][0-9]$ ]]; then
        die "failed to download rustup from $RUSTUP_URL (curl exit $curl_status, HTTP ${http_status:-unknown})."
    fi
    [[ -s "$rustup_installer" ]] || die "rustup download succeeded but returned an empty installer."

    sh "$rustup_installer" -y --default-toolchain stable
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
}

main() {
    echo "==> FxLinux installer"

    require_command sudo "Install sudo before running this installer."
    require_command curl "Install curl before running this installer."
    detect_platform
    install_system_packages
    install_rust
    require_command cargo "Rust installation did not provide cargo."

    cd "$SCRIPT_DIR"

    echo "==> Building FxLinux (release)"
    cargo build --release

    echo "==> Wiring FxLinux into the system"
    ./target/release/fxlinux-app install "$@"

    cat <<EOF

==> Done.

Set "FxLinux" as your default audio output (desktop Sound settings, or
\`wpctl set-default <id>\`), then launch the control panel with:

    $SCRIPT_DIR/target/release/fxlinux-app

It will also start automatically on your next login.
EOF
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
