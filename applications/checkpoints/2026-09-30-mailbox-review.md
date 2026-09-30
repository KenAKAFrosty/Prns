# Mailbox correctness and documentation review

This review followed the Network work at `32dc36b13`. It confirmed five source
issues: first-arrival deduplication could suppress later source verification,
the global latest-100 query could hide conversations, overlapping UI reads could
publish stale results, mailbox reads copied unnecessary history, and Inbox
previews attributed unverified messages to saved names without a warning.

## Changes

- The durable and in-memory LXMF services permit only an unverified-to-verified
  upgrade for matching signed message data. The durable transaction retains the
  local record ID, replaces the signature bytes, advances its revision once and
  remains valid after reopening. Further duplicates cannot downgrade it. Learning
  a sender key alone does not reverify saved messages.
- Message pages use a reverse database cursor with an exclusive record-ID bound
  and stop at the matching row limit. Inbox has a separate latest-per-peer query,
  so message volume in one conversation cannot hide another conversation.
- Inbox and conversation history load 50 rows at a time with one lookahead row.
  Refresh revisits the loaded window, including older delivery-state changes.
  Responses and retained refresh callbacks are scoped to the current recipient,
  lifecycle owner and mounted screen.
- Mailbox reads follow durable and process-local LXMF revisions, not every
  network snapshot. The latter also covers active sends, discovered peers and
  health, so it is not a message-only counter. Offline mutations explicitly
  refresh because stopped health reports zero revisions.
- Unverified incoming previews show a warning and qualify the claimed sender.
  A saved alias or an outgoing delivery proof is not sender verification.

The service changes are in `97ecbb137`; app integration is in `eb030e075`.
Product queries remain in native composition with generated TypeScript, Swift
and Kotlin bindings; no app policy
was moved into the general SDK, and core still has no application dependency.
The new native contract requires a matching rebuilt app binary.

## Evidence and limits

Automated checks cover signed invalid/unknown-to-verified repeats, no downgrade,
transactional/reopen consistency, exclusive cursors, quieter conversations beyond
100 busy-peer rows, history beyond 100 messages, stale success/failure/pending
results, recipient/runtime replacement, unmounts and preview warnings. Native
admission tests cover offline reads and the running-generation command path.

| Check | Result |
| --- | --- |
| LXMF service, including redb mailbox | 58 unit and 3 integration tests passed; strict all-target Clippy passed |
| Native composition, host-test | 215 unit and 4 integration tests passed |
| Native composition, host-test + Android | 214 unit and 4 integration tests passed; strict all-target Clippy passed |
| App | All 452 tests in 41 suites, formatting, lint and three TypeScript checks passed |
| Platform bridge | All 68 tests, source-level iOS ownership checks, formatting, lint and TypeScript checks passed |
| Generated bindings | Regeneration/check, 7 generator regressions and aggregate foreign ownership/restart/reset checks passed |
| Repository and packaging | Application dependency boundary and its 5 regressions, route/config checks and web export passed |
| Documentation | 216 local links and 24 Markdown anchors across 18 documents passed |

These are local source/host checks, not hosted CI or Android/iOS device tests.

No new phone build was installed for this review. The
[Network phone results](2026-09-30-network-inspection.md) describe the earlier
binary, not these changes. Follow up with rebuilt Android/iOS retained-mailbox
checks and paging/preview layout checks before claiming device acceptance.

Conversation results have bounded payloads, but finding distinct peers may still
scan the mailbox and retain a set of visited peer IDs. It is not constant-time
or constant-memory in conversation count; an index is a future measured
optimization, not part of this repair. Multiple page queries are not one database
snapshot: new messages can move a conversation before a cursor, and the revision
refresh reconciles the visible window. This is not mailbox synchronization.

Competing Bluetooth connections and status-8 disconnects remain investigation
leads. This review established no transport cause and made no Bluetooth changes.

## Documentation cleanup

The app guide now includes Network and removes obsolete restart-blocker and
upstream-baseline statements. Validation is an evidence index; the roadmap owns
priorities and landing dependencies. Historical validation and completed SDK
design/cutover narratives are preserved in checkpoints, with short redirects
from the former plan paths. Ignored scratch notes are not current authority or
required links for a fresh clone.
