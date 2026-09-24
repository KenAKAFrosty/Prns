# G4 station-mode outage and recovery

The G4 served a complete Hopspot page as a 2.4 GHz station, recovered from a
controlled AP outage, and served the same page from the same running process.
This is a retained-address recovery smoke, not a DHCP-renewal, Internet-failure,
or automatic discovery-to-peer qualification.

## Setup

Starting repository commit: `7f0638955`. Application binary remained the reviewed
5,334,876-byte static MIPS build, SHA-256
`a11baef5eaec55c06bd4d6f0bcab50f5049cf94ed84d92f1172142036ac790ee`.

Heltec A hosted the temporary channel-1 HT20 WPA2/CCMP AP on 172.23.73.1/24;
the G4 joined it and obtained 172.23.73.110/24. Each board ran only one 2.4 GHz
role. The second Heltec remained untouched. Independent wired management and
existing HaLoW interfaces stayed available. DHCP supplied no default route or
DNS server, and no persistent configuration was changed.

Hopspot used `--listen 172.23.73.110:4345 --auto-wifi-device prnssta0` with a
260-second lifetime and a private state directory. The explicit page probe ran
on the AP against the G4's automatic TCP rendezvous listener on port 42699.
The lab station firewall explicitly admitted TCP 4345 and 42699.

## Finding: reply traffic in the lab AP firewall

The first page probe stalled even though association and DHCP succeeded. Capture
on the AP showed SYN-ACKs from 172.23.73.110:42699 to the AP's ephemeral ports;
the AP input rejection counter increased. The temporary AP chain ran before
fw4's ordinary established-connection allowance, so it rejected replies to its
own connections. This was a lab firewall defect, not evidence of a Hopspot
station transport failure.

Adding `ct state established,related counter accept` at the beginning of that
chain made the complete page transfer pass. The checked-in staging generator now
includes that rule. It does not grant new incoming connections to unlisted
ports. A subsequent station-to-AP SSH attempt still failed.

## Clean repeat after correcting the firewall

| Observation | Result |
| --- | --- |
| Before outage, exact page | 3,542 bytes verified; probe RTT 47 ms |
| AP hostapd stopped | G4 reported not connected, carrier 0; AP ping 0/2 |
| AP hostapd restarted | Automatic reassociation; first ping check after a five-second wait passed 2/2 |
| After outage, exact page | Same 3,542-byte page verified; probe RTT 68 ms |
| Hopspot continuity | PID 17811 and Linux process start tick 6775246 unchanged before, during and after outage |
| Identity | Same node-page destination `9f5235af13a915eac4758a68e8cbab31`; only one ready line in the process log |
| Firewall reply counter | 36 packets / 9,113 bytes accepted at the post-transfer sample |
| Discovery after recovery | Six G4 Auto-WiFi IPv6 beacons captured on the AP in ten seconds; zero capture drops |
| Memory snapshot after recovery | MemAvailable 17,748 KiB; no swap |

The AP was kept down through a ten-second wait plus negative connectivity and
station-state checks. The five-second wait after restart bounds only when the
first check was performed; it is not a measured association latency. No Hopspot
or supplicant restart, lease renewal, address replacement or manual reassociation
was needed. These were fresh page sessions, not survival of an in-flight link.

The [lab recipe](../network-lab.md#station-mode-recovery)
describes reproduction. The next qualifications are changed-address/DHCP recovery,
automatic peer establishment and withdrawal, and sustained resource use.
Concurrent AP+STA operation remains unqualified.

## Restoration and checks

Both stages retained the seven-minute rollback watchdog and were stopped through
the same cleanup function after measurements. After netifd completed its
asynchronous restoration, G4's original 2.4 GHz interfaces were down and the
Heltec's original AP was up again. Original addresses/routes remained, all four
UCI configuration hashes matched on both devices, and there were no pending
UCI changes or remaining lab firewall rules/processes/interfaces. The original
G4 Hopspot returned its exact 3,542-byte page with probe RTT 53 ms.

Board wall clocks were unsynchronized; this report uses the host session date.
Script syntax, repository validation registry, and diff whitespace checks passed.
The new established-reply rule was verified directly on hardware; no Rust
implementation changed. Private staged credentials and raw snapshots remain
outside the repository.
