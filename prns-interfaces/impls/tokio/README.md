# Personal RNS (Prns)

This crate is one package in the Personal RNS public Rust graph. Quick overviews, the complete feature guide, API documentation, examples, and the cross-language SDK overview are available at [prns.dev](https://prns.dev) or [reticulum.rs](https://reticulum.rs), and in the [source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual MIT/Apache-2.0 license.

## BLE full-duplex progress

`BluetoothPeer` uses the no-std core's `send_frame_duplex` to receive and forward
frames while keeping one send alive. This avoids mutual stalls when fragmented
sends fill bounded queues in both directions. The adapter retains one additional
`BLE_WIRE_FRAME_LEN` outbound buffer (currently 564 bytes), accepting custody only
after copying the whole frame; completion and TX accounting follow the send result.
The wire format is unchanged. Embassy's fanout uses the same core sender with
serialized access to its shared inbound lane.

Normal and in-flight-send reception both use the core's checked `receive_frame`
boundary. Invalid reported lengths close the peer without forwarding or counting
a partial frame; the same boundary now protects Embassy member reception.
