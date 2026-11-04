# Shared RemoteControl setup

Shared RemoteControl setup provides public setup cards, durable target-access
import and ESP USB enrollment alongside target grants and interactive pairing. The shared code has no
application-specific onboarding, messaging or group-membership dependency.

## Separate connection, authorization and configuration

A controller needs both a grant on the target and trusted target identity/access
information saved locally. Connecting to a transport establishes neither. A
successful firmware update also establishes neither.

Applications can finish initial setup without a board. When someone chooses to
add a board, they can keep its radio configuration, change it deliberately, or
finish configuration later. Failed configuration must not remove saved access.
Use an authenticated request to verify access separately from importing a card.

Ordinary firmware updates preserve the target identity, controller grants and
radio settings. Creating a new identity or replacing ownership is a separate,
explicit provisioning operation. Do not silently re-enroll a controller during
status recovery or reconnect.

## Public setup cards

The allocation-free codec lives in
[`public_card.rs`](../prns-core/src/remote_control/public_card.rs). A controller
card contains a controller public key. A target-access card contains the recipient
controller public key, target public key, explicit authority and request set.
The bounded version-1 binary format uses the `prns-rc:` hexadecimal text carrier.
Literal vectors, malformed-key cases and exact permission round trips are tested.

Cards contain no private keys, signatures or claim of current authorization.
The caller must trust their source. A target-access import verifies the complete
recipient key against the live local controller, rejects a different existing
access record, and returns success only after persistence and activation. An
explicit replacement API remains available for reviewed replacements. Once
accepted, an import continues even if its caller stops waiting.

The headless controller supports card exchange and delegation between independent
controllers through the shared codec and runtime APIs. Raw public-key entry
remains supported. Existing clients need a small adapter to consume cards; card exchange requires a client that implements this format.
Interactive invitation pairing retains its existing protocol and authority model.

## ESP USB enrollment

Capability-gated USB-Auto messages submit a public controller key against the
board's existing target identity. S3, S3FN8 and C6 compositions use the same
bounded enrollment owner and durable grant API. The adapter accepts local USB
requests; it does not forward enrollment through the Reticulum network.

See the [wire specification](../prns-core/src/interfaces/usb_auto/serial-enrollment.md)
and shared Rust/JavaScript vectors. The browser selects the ESP adapter through
its existing board catalog, checks capability and target identity, and exports a
target-access card only after a matching durable `Saved` receipt.

Pending receipts mean startup/save is still in progress. After two minutes,
checking pauses while retaining the owned USB connection and transaction.
**Continue checking** resumes status reads. **Stop checking** releases the
connection; it does not cancel or resubmit authorization. Missing, stale or
mismatched receipts remain unknown. An open page can reconcile its retained
attempt without replaying the grant.

Normal ESP runtime serial logs are disabled because log writes can interrupt
USB-Auto frames. Explicit diagnostic logging builds are not qualified for
simultaneous serial enrollment. Boot diagnostics remain available.

## Display and button integration

E290, V3/V3.1, V4/S3R2, V4 R8/S3R8 and T-Beam S3 Supreme expose the shared
**Pairing** menu. They reuse invitation, confirmation and durable-completion
state. V3 uses the S3FN8 SRAM runtime and shared pairing UI without PSRAM.

The E290's Pairing label uses the normal menu font. SX126x startup releases
hardware reset before an unconfigured radio enters standby. That path
does not choose a channel or activate RX/TX. Later standby calls preserve an
already-configured radio's registers; failed or cancelled resets are retried.

S3 startup keeps sealed Wi-Fi restoration and large runtime/LoRa future
construction in separate, bounded phases. This avoids keeping large construction frames live beneath recovery work when
pairing-enabled firmware starts.

## Scope and qualification

The [maintainer checklist](remote-control-maintainer-testing.md) separates
software, build and physical evidence. Headless ESP targets retain serial setup.
Each board must pass its own firmware resource checks before physical acceptance;
a successful host test does not establish embedded memory headroom. This setup path does not implement nRF menu integration, Tracker V2 support,
group delivery or private-key sharing. App consumers use the same public
setup APIs.
