# Application Integration

Treat a Prns node as long-lived application infrastructure. Create it at the
same ownership level as the services that need networking, keep one owner for
its shutdown path, and pass narrower handles or application-specific messages
to feature code.

## Node and event ownership

A node should normally live for the application session, not for one screen,
request, or component render. Repeated construction discards routing knowledge,
reopens interfaces, and consumes more energy than keeping one bounded engine
alive.

Application events and diagnostics are separate single-owner streams. Claim
each stream once near the node owner. If several features need the same event,
the owner should translate it into application-domain data and fan that data
out through the application's own bounded channels. Features should not race to
claim the engine stream or retain resource bodies indefinitely.

## Cancellation and shutdown

Cancellation belongs at the adapter boundary that began the work. Stop issuing
commands, cancel or close claimed streams through the SDK's native mechanism,
wait for foreign blocking calls to return, then release the host. On process
shutdown, stop the node before destroying state used by event handlers.

Commands settle as typed success or failure outcomes. Handle expected network
conditions at the call site. Reserve exceptions or process termination for
violated local contracts, incompatible binaries, or failures the application
cannot recover from.

## Browser permissions

Browser node creation has no permission side effects. WebUSB, Bluetooth, and
local-network actions must remain behind a user gesture and their tagged early
outcomes. A permission denial, unavailable API, disconnected device, or
already-owned stream is application state, not an unhandled exception.

Keep large browser resources as `Blob` values when possible. The browser SDK
slices them into bounded resource segments instead of materializing the entire
file in JavaScript memory.

## Embedded host or shared daemon

Embed the host when one application owns the device's Reticulum role, needs the
lowest latency, or must control interface and identity lifetime directly. This
is the normal shape for firmware, browser tabs, mobile applications, and
single-purpose services.

Use `prnsd` or a shared-instance client when several local processes must share
one identity, one interface set, or one routing view. The daemon becomes the
long-lived node owner; clients connect through the shared-instance boundary
instead of each opening radios and sockets. Android applications that connect
to a separately managed instance should follow the
[Android shared-instance guide](https://github.com/KenAKAFrosty/Prns/blob/main/docs/android-shared-instance-client.md).

The [example catalog](examples.md) shows the engine-level ownership shape. The
[host-contract guide](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/README.md) describes how each language
preserves single ownership, cancellation, settlement, and bounded resources.
