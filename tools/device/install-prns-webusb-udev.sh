#!/usr/bin/env bash
set -euo pipefail

rule_path="/etc/udev/rules.d/60-prns-webusb.rules"
rule='SUBSYSTEM=="usb", ATTR{idVendor}=="1209", ATTR{idProduct}=="0001", MODE="0660", TAG+="uaccess"'

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "Prns WebUSB udev setup is only needed on Linux." >&2
  exit 1
fi

tmp_rule="$(mktemp)"
trap 'rm -f "$tmp_rule"' EXIT
printf '%s\n' "$rule" > "$tmp_rule"

sudo install -m 0644 "$tmp_rule" "$rule_path"
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=usb --attr-match=idVendor=1209 --attr-match=idProduct=0001

echo "installed $rule_path"
echo "unplug and replug the Prns USB Auto device, then retry Chrome WebUSB"
