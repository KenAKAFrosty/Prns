# Runtime entropy

Prns turns each platform's audited hardware or operating-system entropy source
into one continuous runtime random stream. Platform bring-up constructs
`RuntimeEntropy<S>` through an `EntropySource`; protocol and application code
then uses only `fill_random`. Identity/bootstrap APIs and Embassy host and
interface boundaries require that branded stream or an opaque handle to it, so
an arbitrary callback cannot accidentally become the production random source.

## Initialization and ownership

`RuntimeEntropy::try_new` must obtain a complete 32-byte seed before a generator
exists. Failure is returned to the platform and no random output is available.
Core trusts a successful `EntropySource` call: it cannot measure physical
entropy, and an all-zero result is not rejected as though a statistical test
could prove source quality.

The resulting ChaCha20 generator owns both its secret state and its source. It
is neither cloneable nor serializable and does not expose a raw-seed
constructor. `with_source` consumes the generator when boot moves to another
provider, preserving the continuous secret stream without copying it. Embassy
installs that generator once behind a mutex and distributes copyable
`EntropyHandle` values; copying a handle does not copy the generator.

Standard/Tokio hosts seed from the operating-system CSPRNG. A manifold owns a
non-cloneable stream, while the few thread-local consumers seed an isolated
stream lazily. Continuing Prns inside a process produced by raw Unix `fork` is
unsupported because inherited generator state would be duplicated; spawning a
fresh executable remains supported.

## Reseeding

Runtime output comes from ChaCha20. After each 64 KiB output window, the next
non-empty fill attempts to obtain 32 fresh bytes. A successful reseed also
draws 32 hidden continuity bytes from the existing stream and derives the next
seed with this fixed transcript:

```text
fresh       = 32 bytes from EntropySource
continuity  = 32 hidden bytes from the existing ChaCha20 stream
PRK         = HKDF-Extract(
                salt = fresh,
                IKM  = continuity
              )
new_seed    = HKDF-Expand(
                PRK,
                "personal-rns/csprng/reseed/v1",
                32 bytes
              )
```

The fresh-source read happens before the existing generator advances. If it
fails, the prior secure stream remains in service. A scheduled failure marks
`ReseedHealth::Deferred` and opens another full 64 KiB retry window so a failing
hardware source is not hammered on every request. A later success restores
`Healthy`. Health reports the last reseed result; it does not validate the
physical quality of bytes that a platform reported as successful.

`try_reseed` exposes the same operation for a platform transition. The caller
decides whether that transition is mandatory or opportunistic. An explicit
failure neither replaces nor advances the generator, but a mandatory caller
must withhold output or terminate until reseeding succeeds. ESP radio and nRF
SoftDevice transitions are opportunistic because the already-secure boot
stream carries forward.

The design shares Rust `ThreadRng`'s high-level 64 KiB reseed cadence. It is not
the same implementation and is not described as a FIPS-approved DRBG. Directly
controlled seed, continuity, source, PRK, and derived-seed buffers are erased
where their types support it; this is not a stronger claim that a compiler can
guarantee every historical copy has been erased.

## Platform contracts

| Platform | Initial seed | Runtime source and transition |
| --- | --- | --- |
| Tokio/std | Operating-system CSPRNG | The owned source periodically reseeds from the OS; raw post-fork continuation is unsupported. |
| ESP32-S3 | `TrngSource` enables the documented RNG/ADC entropy path before first-boot node, remote-control, or Auto-BLE identity generation. | Temporary boot ownership is released before board ADC use. The continuous stream is installed after Wi-Fi radio initialization and opportunistically reseeded there and after BLE starts. |
| ESP32-C6 | `TrngSource` is active before node, remote-control, or Auto-BLE identity generation. | The boot source is released before radio ownership changes. The continuous stream is installed after ESP-NOW initialization and opportunistically reseeded there and after BLE starts. |
| nRF52840 with SoftDevice | HAL RNG before identity generation. | `with_source` moves the stream to SoftDevice entropy without cloning it; installation attempts an immediate reseed and periodic draws use that source. |
| nRF52840 T1000-E | HAL RNG before identity generation. | The HAL RNG remains the runtime reseed source. |

Board adapters are the only application code allowed to call the raw hardware
source. A new platform must prove its initialization order by implementing
`EntropySource`, constructing `RuntimeEntropy` before identity/bootstrap work,
and passing only the resulting stream or handle into runtime consumers.

## Personal Hopspot 0.3.7-hotfix.5 remediation

Personal Hopspot ESP32-S3 firmware versions 0.3.7 through 0.3.7-hotfix.4 could
create a node identity and Auto-BLE identity before the documented primary
hardware entropy source was enabled. This affected first boot after a full
erase on Heltec LoRa 32 V4 variants and LilyGO T-Beam Supreme. Devices upgraded
while retaining valid stored identities did not regenerate them and were not
affected by that initialization path.

The remediation for a potentially affected installation is a
**0.3.7-hotfix.5 or later full-erase reflash**. An ordinary sparse update
deliberately preserves stored identity, routes, ratchets, radio configuration,
and provisioning state, so it cannot replace an identity created on the
affected path. Firmware will not silently rotate retained identities: identity
replacement changes the node's public destinations and must remain an explicit
operator action.

A full erase destroys the device's stored identities and other persistent
state. Record anything needed for recovery, verify the exact board, and perform
a full-chip erase before installing 0.3.7-hotfix.5 or later.
Devices whose identities predate 0.3.7 do not need rotation solely because they
later ran an affected release.
