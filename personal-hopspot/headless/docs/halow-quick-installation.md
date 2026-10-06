## 1. Check the device and back it up

SSH in over Ethernet, verify the host key, and run:

```sh
cat /tmp/sysinfo/board_name
cat /proc/sys/kernel/osrelease
df -Pk /overlay /tmp
uci -q changes
```

The board name and kernel should match the table above, and `uci -q changes` should print nothing. Then export the device's configuration with its own backup feature. Keep that file private: it holds passwords and keys.

## 2. Copy the files over

Two things go onto the device:

- **The manager**, the tool that installs the app and can roll it back, at `/etc/hopspot/manager`.
- **A signed app package** (`manifest.json`, `manifest.minisig`, `app.gz`) made for this exact board, in `/tmp/hopspot-package`.

Upload everything to a private folder such as `/tmp/hopspot-upload`, including the manager bundle's US radio profile as `radio-profile.json`. Compare hashes on the device before moving files into place.

Then define this helper, which every later command uses, and inspect:

```sh
manager() {
    /etc/hopspot/manager --root /etc/hopspot \
        --max-compressed-bytes 2097152 --max-executable-bytes 4194304 \
        --flash-reserve-bytes 262144 --ram-reserve-bytes 8388608 "$@"
}
manager inspect --profile /tmp/hopspot-upload/radio-profile.json
```

`inspect` only reads. It reports what it found as JSON and stops on a device it does not support.

## 3. Stage the app

Save this as `/etc/hopspot/config.json` with mode 0600. The radio starts off so the first test is wired only:

```json
{
  "listen": "[::]:4242",
  "tcp_mode": "Gateway",
  "radio": "Disabled"
}
```

```sh
manager stage --package /tmp/hopspot-package --trial-launches 3
```

Write down the `revision` and `executable_sha256` it returns. You need them to confirm later.

## 4. Pair your controller and test

The controller is the Remote Control client on your computer: the `controller` example in `personal-hopspot/headless`. Create its identity:

```sh
controller --state-dir /private/controller-state identity
```

In the SSH session, set the `public_key` it prints as `CONTROLLER_PUBLIC_KEY` and start a three-minute test run:

```sh
manager qualify --config /etc/hopspot/config.json --ram-directory /tmp/hopspot \
    --controller-public-key "$CONTROLLER_PUBLIC_KEY" \
    --controller-access application-probe --run-for 180
```

Save the `target_key` from its output. While the test runs, check the device from your computer, with its address and port as `WIRED_ENDPOINT` and that key as `TARGET_PUBLIC_KEY`:

```sh
controller --state-dir /private/controller-state invoke \
    --tcp "$WIRED_ENDPOINT" --target-key "$TARGET_PUBLIC_KEY" --action build
controller --state-dir /private/controller-state invoke \
    --tcp "$WIRED_ENDPOINT" --target-key "$TARGET_PUBLIC_KEY" --action interfaces
controller --state-dir /private/controller-state invoke \
    --tcp "$WIRED_ENDPOINT" --target-key "$TARGET_PUBLIC_KEY" \
    --action app-message --message-hex 0101
```

All three should answer. Each test run uses one of the three trial launches.

## 5. Start the service and confirm

Install the bundle's `openwrt/hopspot` script as `/etc/init.d/hopspot` with mode 0700, enable and start it, and run the same three checks again. If they pass, confirm:

```sh
manager confirm --revision "$APP_REVISION" \
    --executable-sha256 "$APP_EXECUTABLE_SHA256"
```

If they fail, stop the service and run `manager rollback`. A trial left unconfirmed ends on its own after three launches.

## 6. Turn on HaLoW

The radio change is a separate trial with its own safety net: unless you confirm within five minutes, the device restores its original radio settings and reboots. The tested profile is US only, so check it against your region first. Keep Ethernet connected, and have a second HaLoW Hopspot in range to test against.

Stop the service, set `radio` in the config to the HaLoW device, and start it again:

```json
"radio": { "HaLow": { "device": "wlan0", "scope": "primary-halow" } }
```

Install the bundle's `openwrt/hopspot-radio-recovery` script as `/etc/init.d/hopspot-radio-recovery` and the radio profile as `/etc/hopspot/radio-profile.json`, then prepare the change:

```sh
chmod 700 /etc/init.d/hopspot-radio-recovery
/etc/init.d/hopspot-radio-recovery enable
manager radio prepare --profile /etc/hopspot/radio-profile.json \
    --recovery-seconds 300 --trial-boots 1
```

Write down this `revision` and `profile_sha256`, then apply:

```sh
manager radio apply --revision "$RADIO_REVISION" \
    --profile-sha256 "$RADIO_PROFILE_SHA256"
```

Run the controller's `announce` action on each board so they find each other over the radio. Then repeat the step 4 checks against this board through the other one. If they pass, confirm:

```sh
manager radio confirm --revision "$RADIO_REVISION" \
    --profile-sha256 "$RADIO_PROFILE_SHA256"
```

If they fail, run `manager radio rollback` with the same values or let the timer run out. `manager radio status` should end at `Restored`.

## Keep these afterward

Your backups, the app package, both `revision` and hash pairs, the controller's public key and the device's `target_key`. Leave the radio recovery service installed.

Not tested yet: power loss in the middle of an install, updating the manager itself, and other regions.
