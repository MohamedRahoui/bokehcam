#!/usr/bin/env bash
# Load the v4l2loopback kernel module for this boot only (nothing is written
# to /etc). Use scripts/install.sh to make it permanent.
#
# Usage: scripts/load-module.sh [video_nr] [label]
set -euo pipefail

video_nr="${1:-10}"
label="${2:-bokehcam}"

if ! modinfo v4l2loopback >/dev/null 2>&1; then
    echo "v4l2loopback is not installed (Arch: sudo pacman -S v4l2loopback-dkms)" >&2
    exit 1
fi

if lsmod | grep -q '^v4l2loopback'; then
    echo "reloading v4l2loopback (close any app using the virtual camera first)"
    sudo modprobe -r v4l2loopback
fi

# exclusive_caps=1 makes the device look like a plain webcam, which browsers require.
sudo modprobe v4l2loopback devices=1 video_nr="$video_nr" card_label="$label" exclusive_caps=1
echo "virtual camera ready: /dev/video$video_nr ($label)"
