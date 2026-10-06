# Set up Hopspot on the Heltec HT-HD01-V2

Hopspot installs as an app on the Linux system your HT-HD01-V2 already runs. The vendor firmware and radio calibration stay in place.

**Development preview.** Signed installs, reboots, Remote Control, node pages and automatic radio rollback have passed on two HT-HD01-V2 boards and on a ThinkNode G4. There is no public download or one-click installer yet: developers build from the repository and install over SSH.

## Before you start

Your board should match the tested ones. Other Heltec HaLoW products are not covered:

| What to check | Tested HT-HD01-V2 |
| --- | --- |
| Board name | `Heltec,HT-HD01-V2` |
| Vendor image | OpenWrt 23.05.5, Morse 2.8.5-20250924 |
| Kernel | 5.15.167 |
| Radio profile | US only: 924 MHz, 8 MHz wide, 18 dBm |

You need wired Ethernet to the board for the whole install, SSH access, and a checkout of the Prns repository. Build the app with `build.hopspot.g4` (both boards run the same app) and its manager with `build.hopspot.appliance`, both through `./tools/prns`.

Look the board's address up: the sticker on the case may not match the mode or address it is using now. If you reach it at an IPv6 link-local address, include your computer's Ethernet interface (the scope) in the SSH address and the controller endpoint.

Never load these files through LuCI's firmware upgrade or `sysupgrade`.
