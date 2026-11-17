#!/usr/bin/env bash
# Build and install bokehcam for the current user.
#
#   scripts/install.sh              build, install, download models, load the kernel module
#   scripts/install.sh --service    also start bokehcam at login (systemd user unit)
#
# What it changes (scripts/uninstall.sh reverts all of it):
#   ~/.local/lib/bokehcam/                     the two programs and their runtime library
#   ~/.local/bin/bokehcam, bokehcam-gui        links to them
#   ~/.local/share/applications/bokehcam.desktop   app-launcher entry for the window
#   ~/.local/share/bokehcam/models/            person-detection models
#   ~/.config/bokehcam/config.toml             created only if missing
#   /etc/modules-load.d/bokehcam.conf          load v4l2loopback at boot
#   /etc/modprobe.d/bokehcam.conf              virtual camera number and name
#   ~/.config/systemd/user/bokehcam.service    only with --service
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
video_nr="${BOKEHCAM_VIDEO_NR:-10}"
label="${BOKEHCAM_LABEL:-bokehcam}"
with_service=0
[[ "${1:-}" == "--service" ]] && with_service=1

lib="$HOME/.local/lib/bokehcam"
bin="$HOME/.local/bin"
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"

say() { printf '\n==> %s\n' "$*"; }

say "System packages"
if command -v pacman >/dev/null; then
    sudo pacman -S --needed --noconfirm v4l2loopback-dkms clang
else
    echo "Not an Arch-based system: install v4l2loopback (0.13 or newer) and clang yourself."
    modinfo v4l2loopback >/dev/null 2>&1 || { echo "v4l2loopback not found" >&2; exit 1; }
fi
command -v cargo >/dev/null || { echo "Rust is required: https://rustup.rs" >&2; exit 1; }

say "Build (the first build takes a few minutes)"
(cd "$repo" && cargo build --release --locked)

say "Install to $lib"
mkdir -p "$lib" "$bin" "$data_home/applications"
install -m 755 "$repo/target/release/bokehcam" "$repo/target/release/bokehcam-gui" "$lib/"
# The graphics-card backend of the model runtime is a shared library that
# must sit next to the programs.
find "$repo/target/release" -maxdepth 1 -name '*.so*' -exec install -m 644 {} "$lib/" \;
ln -sf "$lib/bokehcam" "$bin/bokehcam"
ln -sf "$lib/bokehcam-gui" "$bin/bokehcam-gui"
sed "s|@BOKEHCAM_GUI@|$bin/bokehcam-gui|" "$repo/packaging/bokehcam.desktop" \
    > "$data_home/applications/bokehcam.desktop"

say "Models"
"$bin/bokehcam" models fetch

say "Config"
if [[ -e "$config_home/bokehcam/config.toml" ]]; then
    echo "keeping existing $config_home/bokehcam/config.toml"
else
    "$bin/bokehcam" init
fi

say "Kernel module (loaded now and at every boot)"
echo "v4l2loopback" | sudo tee /etc/modules-load.d/bokehcam.conf >/dev/null
echo "options v4l2loopback devices=1 video_nr=$video_nr card_label=\"$label\" exclusive_caps=1" |
    sudo tee /etc/modprobe.d/bokehcam.conf >/dev/null
if ! "$bin/bokehcam" devices 2>/dev/null | grep -q "/dev/video$video_nr"; then
    "$repo/scripts/load-module.sh" "$video_nr" "$label"
fi

if (( with_service )); then
    say "systemd user service"
    mkdir -p "$config_home/systemd/user"
    sed "s|@BOKEHCAM@|$bin/bokehcam|" "$repo/packaging/bokehcam.service" \
        > "$config_home/systemd/user/bokehcam.service"
    systemctl --user daemon-reload
    systemctl --user enable --now bokehcam.service
    echo "logs: journalctl --user -u bokehcam -f"
fi

say "Done"
"$bin/bokehcam" doctor || true
(( with_service )) || echo "Start it with \`bokehcam run\`, or open the bokehcam window."
