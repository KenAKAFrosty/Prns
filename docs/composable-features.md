# Composable capabilities

Prns separates networking, storage, identity ownership and group communication.
Applications select these libraries and own their configuration, composition,
presentation and execution lifetime. No library requires a chat application.

## Choose an owner

| Need | Owner and guide | Consumer responsibility |
| --- | --- | --- |
| Reticulum routing, Links and interfaces | [Rust facade](../personal-rns/README.md), [host runtime](../prns-host/impls/runtime/README.md) | Interface selection, host lifetime and service admission |
| Private files and bounded blocking work | [Storage](../prns-storage/README.md) | Domain schemas, transactions and recovery |
| Keys and named-identity lifecycle | [Identities](../prns-identities/usage.md) | Storage paths, limits, service configuration and activation |
| Signed identity and service claims | [Contact cards](../prns-contact-card/README.md) | Trust, consent and address-book policy |
| Encrypted records, enrollment and device receipts | [Groups](../prns-groups/usage.md) | Group profiles, local budgets and optional network scheduling |
| Text within group records | [Text profile](../prns-groups/profiles/text/README.md) | Conversation presentation and history policy |
| Identity and group assembly over shared resources | [Service runtime](../prns-service-runtime/README.md) | Feature selection, admission policies and embedding lifetime |
| Expo and React Native host access | [Mobile SDK](../prns-react-native/README.md) | Selected native image, platform permissions and application lifecycle |

Storage has no domain or host dependency. The Rust group owner can operate
without a named-identity catalog or running host. Optional networking borrows a
host client; it does not construct another node. Portable group policy, storage,
security and record layers are described in the
[group architecture](../prns-groups/docs/design.md).

## Storage choices

Identity catalogs and private keys use bounded snapshots and checked private-file
publication. These small owner-controlled files keep key custody and catalog
recovery independent of a database backend. Group record and history stores use
redb where indexed queries and atomic updates across related records are needed.

Both use the shared storage crate for private-file operations and serialized
blocking work. The domain owns its schema, transaction boundaries and recovery
policy; sharing storage mechanics does not require every domain to use the same
file format or database.

## Construction and lifetime

Rust consumers may construct a domain owner directly or attach it to an existing
serialized storage lane. Composition must retain the owners and dependencies,
enforce identity admission, and drain accepted work before releasing resources.
Borrowed clients do not acquire authority to stop another component's owner.

The service runtime coordinates the existing identity, group and storage owners;
it does not introduce another domain store or copy their client APIs. Its
`groups-network` factory borrows an embedding's host and executor. With
`standalone` selected, it can construct and retain those resources itself.

Standalone local readiness is independent of network readiness. Preparing local
data does not require a running host. Stopping networking drains its attachments
while keeping the same local clients and saved data available; restarting creates
a new host generation. Full shutdown is separate: the owner fences admission,
drains domain work and joins owned resources before reporting cleanup complete.
Retain that completion handle when teardown is interrupted or fails; releasing a
client is not proof of cleanup.

Creating new state and opening existing state are explicit choices. Opening
validates the persisted identity binding and supported schema; failure does not
authorize replacement, migration or deletion. Retain an exact request across an
uncertain result rather than issuing the same action under a new identifier.

Stored conversation count, per-group member capacity and the active working set
are separate policies. The default catalog has no conversation-count quota;
the default eight-member profile is selectable, not a universal ceiling.
Byte, queue and transaction budgets still constrain admission.

Protocol delivery expiry and retained local history have separate lifetimes.
Select the optional archive when history must remain until explicit deletion.
Archive admission rejects new records when its budgets are full; it does not evict
history. A device receipt proves durable acceptance by that endpoint incarnation,
not that a person read the message. Applications own presentation and deletion.

## Independent foreign consumers

The [identity bindings](../prns-identities/bindings/uniffi/README.md) provide
standalone key ownership and an optional catalog factory; their
[Swift example](../prns-identities/bindings/uniffi/tests/swift_conformance.swift)
exercises creation, leases and reopening.

The [group bindings](../prns-groups/bindings/uniffi/README.md) provide a local
factory without text or networking; their
[Swift example](../prns-groups/bindings/uniffi/tests/swift_conformance.swift)
uses keys and groups from the same selected native image.
Do not exchange opaque handles between separately loaded images.

The [host bindings](../prns-host/bindings/uniffi/README.md) expose owned sessions,
borrowed clients and optional saved connections. Each owning guide supplies its
generation and qualification commands. Host tests do not establish radio,
mobile-background or physical-device behavior.

The [mobile SDK](../prns-react-native/README.md) selects the reusable host image
by default. Product compositions can supply another image through its provider
configuration while reusing the same mobile integration. JavaScript leases and
platform callbacks remain SDK-owned; applications choose installation paths,
permissions and startup policy. An owned host session can stop its host; a
borrowed host client can only release its own resources. Domain APIs remain in
their owning bindings, without adding product methods to the SDK wrapper.

## Chat application

The [chat application](../applications/README.md) selects identities, groups,
archived text history and saved host connections in one generated native image.
It owns installation policy, permission recovery, private import/reset flows,
contacts, drafts, navigation and optional remote-node controls. Its
[product guide](../applications/docs/product-scope.md) describes those workflows
and limits; the [validation guide](../applications/docs/validation.md) separates
source and image checks from physical-device evidence.

The product retains local data independently of network readiness. Stopping
networking leaves saved contacts and history available. Its platform module
supplies operating-system integration while the reusable SDK owns generic
transport mechanics; domain libraries remain usable without the application.
