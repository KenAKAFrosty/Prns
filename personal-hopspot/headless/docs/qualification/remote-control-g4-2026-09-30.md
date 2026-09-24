# Explicit controller qualification — 2026-09-30

Replaces the automatic-announcement experiment with controller-driven operation.
This was a temporary application trial, not a persistent installation or firmware flash.

## Build and setup

Dirty tree based on `8b0975ad6a14eb126f9fb36e24fb7ca05054444f`; features
`wifi-halow,websocket,wifi-auto`. Static MIPS32r2 O32 application: 5,356,124 bytes,
SHA-256 `424650439cecd1e458015baf57b18f6a30b8b45e867acecf4f67f27683833739`.

G4 station and Heltec AP used the isolated 2.4 GHz network-lab setup, independent
wired management, fresh application state, scoped firewall rules and seven-minute
restoration timers. Controller public identity hash:
`6f4dd0975c5cbf521f0704a573ae23e8`. Both targets received the full public identity
as an Operator grant for exactly Describe and AnnounceSelf. The controller's
private identity stays in the ignored local `scratch/hopspot-lab-controller`
directory, outside the application bundle. The third board was not modified.

## Observations

| Check | Result |
| --- | --- |
| Before any control command | 15-second capture on G4 prnssta0 UDP 42671: zero packets, zero kernel drops |
| Cold control bootstrap | Explicit path request, authenticated link, Describe succeeded on both fresh targets |
| Describe | G4 39 ms; Heltec 52 ms; exactly Describe and AnnounceSelf available |
| Untrusted controller | AnnounceSelf failed with `Exchange(Request(Failed(Timeout)))`; no successful operation |
| Authorized AnnounceSelf | G4 53 ms; Heltec 51 ms; controller observed each node-page announce |
| Routed page through G4 to Heltec | Exact 3,542-byte page, 53 ms |
| Routed page through Heltec to G4 | Exact 3,542-byte page, 115 ms |
| G4 restart | Node-page identity and target public key unchanged; same controller Describe succeeded, 41 ms |
| Restore | Network, wireless, firewall and DHCP hashes unchanged on both; original addresses/routes restored; no pending UCI changes or lab firewall rules |
| Original G4 service | Exact 3,542-byte page on port 4242, 46 ms after restoration |

A second 15-second capture after traffic contained one 99-byte UDP payload. It was
not captured with payload decoding, so it is **unclassified**, not proof of total
post-command silence. There was no periodic stream. Removal of autonomous policy
is additionally verified in source and ESP32 composition tests; this short capture
alone does not establish every future idle behavior.

The denied request's timeout matches the source contract: all
`RemoteControlAdmitError` variants become `Decline::Ignore`. It does not prove
physical absence of every frame, and callers cannot distinguish denial from loss
by that error alone. Better developer-facing diagnostics remain API work.

## Scope and retained evidence

Raw logs and controller results: local `/private/tmp/hopspot-control-evidence`.
Private stage scripts and radio credentials are not checked in. Device-side state
from the temporary trials is retained; the original service was preserved.
Future lab runs should provision the same controller and issue explicit commands.

This trial validates control over wired TCP and commanded announcements/routing
across Auto-WiFi. It does not requalify HaLoW over-air behavior, grant administration,
WebSocket control clients, persistent service installation, or ESP32 firmware on
hardware. ESP32 changes here remove boot/pairing announcement triggers and are
covered by native composition tests only.
