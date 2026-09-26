# Response-size accounting

Status: identified follow-up; behavior unchanged by the Resource simulation slice.

## Observed behavior

The mixed Tokio/Embassy Resource capstone found that a 1,200-byte application
response is rejected with `maximum_response_bytes = 1200`, but succeeds when the
limit includes `RESPONSE_WIRE_OVERHEAD`. The current test deliberately names this
an envelope limit rather than claiming it is an application-payload limit.

- [Resource admission](../src/routing/links/resources/receive/gate.rs) compares
  the advertised uncompressed `data_bytes` against the pending request's limit.
- [Packet admission](../src/routing/ingress/links.rs) compares the parsed response
  body's length after unconditionally subtracting two bytes. This is not a
  general MessagePack binary-header parser and also affects raw application data.
- [Resource conclusion](../src/routing/links/resources/receive/conclude.rs) strips
  the response envelope, with compatibility for legacy raw-body responses.
  Metadata and segmented responses have additional accounting paths.
- Embassy completion buffers retain application response bytes, not the outer
  request/response envelope. Their capacity is currently also used as the core
  limit, leaving less usable capacity for enveloped Resource responses.

## Required scope of a correction

Define one explicit public byte-counting contract before changing admission.
Separate early allocation protection from final delivered-payload validation;
loosening an advertisement check alone would admit oversized legacy raw bodies.
Keep the authoritative accounting in shared core, with host completion buffers
enforcing their own actual storage capacity rather than compensating for wire
headers independently.

Cover exact limits, one-byte overflow, zero and unlimited bounds, raw bytes,
MessagePack bin8/bin16/bin32, canonical and legacy Resource envelopes, metadata,
compression, and multi-segment responses. No prefix may be silently subtracted
from arbitrary application data. Check overflow-safe advertised bounds and
bounded allocation before receiving an untrusted transfer.

Extend owner tests and the mixed-runtime capstones with exact Embassy completion
capacity boundaries. Verify typed refusal, no truncated or partial successful
response, one terminal result, receipt cleanup, and subsequent link usability.
Preserve Remote Control's fixed response bounds and stock-Reticulum wire
interoperability. Audit reusable native/Node.js/WASM client semantics before
publishing a changed limit contract; do not introduce a transport-specific fix.
