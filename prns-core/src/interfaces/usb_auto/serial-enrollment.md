# Local serial controller enrollment

This optional extension keeps UsbAuto protocol version 2, the Hello/Ack/Data
messages and RNS serial framing unchanged. HelloAck capability bit `0x02`
advertises enrollment. Legacy peers ignore the extra capability. New control
messages are never delivered to either host or device network data seams.
Devices must opt in and complete a local Hello handshake before serving them.

Each payload is enclosed by `0x7e`; payload bytes `0x7e` and `0x7d` are escaped
as `0x7d` followed by the byte XOR `0x20`. Integers are little endian.

| Kind | Body | Body bytes |
| --- | --- | --- |
| `0x04` authorize | Transaction `u32`, controller full public key, expected target full public key | 132 |
| `0x05` query status | Nonzero query nonce `u32` | 4 |
| `0x06` status reply | Query nonce `u32`, existing USB status receipt | 73 |

The existing 69-byte receipt is a state byte (`0` Idle, `1` Pending, `2` Saved,
`3` Failed), transaction `u32`, and full 64-byte target public key. Idle uses
transaction zero. Public keys contain the 32-byte encryption key followed by
the 32-byte signing key. Both the controller keys and the complete expected
target identity are checked before accepting authorization. The fixed grant is
Administrator with all locally supported request permissions.

Authorization produces an immediate status reply with nonce zero. Rejection
produces an ephemeral Failed receipt for the submitted transaction and current
target key without changing the durable owner's state. Accepted submissions
return the owner's current state, normally Pending. Status queries use a fresh
nonzero nonce and only read state; they never submit or replay authorization.

Clients first check capability and query the full target key. On explicit user
authorization they choose a fresh transaction different from the observed and
locally retained transactions, pin that target in one authorize frame, and await
its nonce-zero reply matching transaction and full target key. Subsequent polls
match their fresh nonce, transaction, and target. A missing reply is an uncertain
outcome: recovery queries only; it never automatically sends authorization again.
An idle or different transaction after reconnect cannot settle the old attempt.
Clients continue Hello heartbeats while polling (or send Hello before each query)
to maintain existing protocol host presence. A definitive Failed response closes
that attempt; a later query of the owner's historical receipt must not revive it.

The durable owner must retain the accepted transaction plus complete grant,
return retained status for exact duplicate submissions, reject the same
transaction with different grant contents, reject a new transaction while work
is pending, and report Saved only after durable storage commit. A completed
duplicate must not renew a grant revoked later. This retained receipt may be
bounded to the latest transaction and may be lost at reboot; it is a historical
save receipt, not proof of current access. Explicit new authorization uses a new
transaction. No enrollment operation changes radio configuration or identity.

The largest enrollment payload is 133 bytes, at most 268 framed bytes. All
control lengths are exact. `enrollment-vectors.json` is consumed by core and
browser tests to keep framing and wire layouts aligned.

The transport must be the sole runtime writer of the serial byte stream. Raw
console logs can interrupt a framed receipt even if each protocol write begins
with one complete buffer. ESP's native USB HAL writes in 64-byte chunks, so normal
ESP builds disable runtime serial logging; diagnostic builds that share this
stream cannot claim serial enrollment or network qualification.
