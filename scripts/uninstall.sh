#!/usr/bin/env bash
# Revert scripts/install.sh. Keeps your config unless --purge is given.
set -euo pipefail

purge=0
[[ "${1:-}" == "--purge" ]] && purge=1
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"

systemctl --user disable --now bokehcam.service 2>/dev/null || true
rm -f "$config_home/systemd/user/bokehcam.service"
systemctl --user daemon-reload 2>/dev/null || true

rm -f "$HOME/.local/bin/bokehcam" "$HOME/.local/bin/bokehcam-gui"
rm -rf "$HOME/.local/lib/bokehcam" "$data_home/bokehcam"
rm -f "$data_home/applications/bokehcam.desktop"
sudo rm -f /etc/modules-load.d/bokehcam.conf /etc/modprobe.d/bokehcam.conf
sudo modprobe -r v4l2loopback 2>/dev/null || echo "v4l2loopback still in use; it will be gone after a reboot"

if (( purge )); then
    rm -rf "$config_home/bokehcam"
else
    echo "kept $config_home/bokehcam (use --purge to remove it)"
fi
echo "bokehcam removed. The v4l2loopback-dkms and clang packages were left installed."
