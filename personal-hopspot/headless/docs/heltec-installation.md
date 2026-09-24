# Heltec HT-HD01-V2 application installation

Install Hopspot on the working vendor Linux system. Choose the exact HT-HD01-V2;
this does not qualify other Heltec HaLoW products or regional variants.

**Development preview:** the common static application passed authenticated
Remote Control, exact HaLoW pages, supervised reboot and protected radio rollback
on two HT-HD01-V2 devices and a ThinkNode G4. There is no signed public download
or unattended install button yet. Follow the shared guided development flow below.

| Inspected property | Qualified Heltec boards |
| --- | --- |
| Vendor board name | `Heltec,HT-HD01-V2` |
| Vendor image / kernel | OpenWrt 23.05.5, Morse 2.8.5-20250924 / 5.15.167 |
| Application | Static MIPS32r2 little-endian O32, soft float |
| Regional radio profile | Explicit US 924 MHz / 8 MHz, MCS2, long guard, 18 dBm |

Use independent Ethernet management, authenticated SSH and private backups.
Discover the actual address and verify its host key. One inspected board uses
scoped IPv6 link-local management: its application listener must accept IPv6, and
the controller endpoint needs the computer's Ethernet scope. Do not assume the
factory AP/station sticker identifies the current runtime mode or management IP.

Developers use `build.hopspot.g4` for the common MIPS application and
`build.hopspot.appliance` for its manager/assets through `./tools/prns`. The G4
name identifies the initial build recipe. Development manager checksums are not
release authentication; the separate application package must be signed under
the manager's trust root. Keep vendor firmware and calibration. Never use LuCI
firmware upgrade or `sysupgrade` for these application files.

[Review compatibility, hardware evidence and remaining gates](https://github.com/KenAKAFrosty/Prns/blob/main/personal-hopspot/headless/docs/halow-deployment.md).
