# ThinkNode G4 application installation

Install Hopspot as an application on the working vendor Linux system. Choose the
exact ThinkNode G4; this is not a firmware-upgrade target.

**Development preview:** signed application slots, supervised reboot, pinned
Remote Control identities, exact pages and protected HaLoW rollback passed on
this G4 and two Heltec boards. There is no signed public download or unattended
install button yet. The procedure below is the shared guided development flow.

| Inspected property | Qualified G4 |
| --- | --- |
| Vendor board name | `morse,ekh03v3` |
| Vendor image / kernel | OpenWrt 1.1 Morse-2.6.13 / 5.15.150 |
| Application | Static MIPS32r2 little-endian O32, soft float |
| Regional radio profile | Explicit US 924 MHz / 8 MHz, MCS2, long guard, 18 dBm |

You need independent Ethernet management, authenticated SSH and private backups.
Discover the actual address and verify its host key. A USB Ethernet adapter
provides network access; it is not a qualified programming interface. Other
images, regional SKUs and USB serial recovery require separate qualification.

Developers build the application with `build.hopspot.g4` and its manager/assets
with `build.hopspot.appliance` through `./tools/prns`. Development manager bundles
have checksums, not release authentication. The manager verifies the separate
signed application package. Never use LuCI firmware upgrade or `sysupgrade` for
these files. Keep vendor firmware and calibration.

[Review compatibility, hardware evidence and remaining gates](https://github.com/KenAKAFrosty/Prns/blob/main/personal-hopspot/headless/docs/halow-deployment.md).
