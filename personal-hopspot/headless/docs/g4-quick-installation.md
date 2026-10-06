# Set up Hopspot on the ThinkNode G4

Hopspot installs as an app on the Linux system your G4 already runs. The vendor firmware and radio calibration stay in place.

**Development preview.** Signed installs, reboots, Remote Control, node pages and automatic radio rollback have passed on this G4 and on two Heltec HT-HD01-V2 boards. There is no public download or one-click installer yet: developers build from the repository and install over SSH.

## Before you start

Your G4 should match the tested one:

| What to check | Tested G4 |
| --- | --- |
| Board name | `morse,ekh03v3` |
| Vendor image | OpenWrt 1.1 Morse-2.6.13 |
| Kernel | 5.15.150 |
| Radio profile | US only: 924 MHz, 8 MHz wide, 18 dBm |

You need wired Ethernet to the G4 for the whole install, SSH access, and a checkout of the Prns repository. Build the app with `build.hopspot.g4` and its manager with `build.hopspot.appliance`, both through `./tools/prns`.

Never load these files through LuCI's firmware upgrade or `sysupgrade`.
