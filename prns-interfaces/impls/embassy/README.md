# Personal RNS

This crate is one package in the Personal RNS public Rust graph. The complete
feature guide, API documentation, examples, and cross-language SDK overview are
maintained at [reticulum.rs](https://reticulum.rs) and in the
[source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual
MIT/Apache-2.0 license.

## Embedded LoRa channel access

The SX126x transport uses work-conserving CSMA/CA shaped for Reticulum and
RNode interoperability. Each pending logical packet draws one millisecond-
resolution ticket from a 15-slot profile-derived self-airtime band. The channel
must remain clear for a real two-slot DIFS before that ticket advances. Busy,
unknown, preamble, header, CRC-error, or RSSI evidence restarts DIFS and freezes
the residual ticket; it does not redraw or escalate it. If the final CCA is busy
after a ticket matures, only a fresh one-slot tie-break tail is selected after
the next DIFS.

Successfully decoded peer frames earn exact PHY airtime, accumulated from any
number of senders and capped at one transmit-opportunity quantum. Earned airtime
accelerates only the ticket countdown continuously from 1x to 3x. Other
channel-busy evidence is deliberately conservative but earns no priority, so a
false preamble or corrupt frame cannot manufacture seniority. Age starts only
while local traffic is pending and is consumed on a win or reset when the
backlog empties, the interface is disabled, or the radio profile changes.

After winning, the transport sends complete queued packets immediately and in
order while the next packet fits. The quantum is the smallest integral multiple
of `2 * time_on_air(255)` that is at least 42 slots. A split packet is an atomic,
contiguous two-frame transmission. The radio releases as soon as the FIFO is
empty, duty policy blocks the next logical packet, a radio error occurs, or the
next packet would exceed the quantum; it never waits to fill unused airtime.
After a quantum-limited opportunity, the continuation uses band zero with one
quantum of earned age, while older contenders keep their residual tickets.

| Preset | Slot | Maximum logical packet | TX quantum |
|---|---:|---:|---:|
| ShortFast | 24 ms | 0.403 s | 1.209 s |
| MediumFast | 25 ms | 1.273 s | 1.273 s |
| LongFast | 99 ms | 4.228 s | 4.228 s |
| LongSlow | 100 ms | 28.407 s | 28.407 s |

The active packet and packed 6 KiB FIFO are unchanged. Scheduling uses only
fixed-size scalar state and does not allocate or add packet-sized storage.
Fairness assumes contenders can detect one another; hidden-terminal mitigation
is outside this scheduler.
