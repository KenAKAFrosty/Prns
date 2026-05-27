# Personal RNS — Build Ethos

Navigation: [Suite README](../README.md)

Status: foundational directive

> **The two sentences to remember:**
> **Port the contract, not the implementation. Build one pure engine, and let each platform bring a thin host.**

This document is the *why*. It is meant to be read in ten minutes and to leave
you able to make the right call on a file you've never seen, without asking.
The technical shapes (concrete types, traits, module layout) come later, and
only if a document is even the right medium for them. Principles first.

---

## 1. What we are building

The full picture, in one line:

> "performance-focused drop-in-replacement for rnsd, with full embedded
> support, plus ecosystem libraries including iOS/SPM, android/maven central,
> and npm; with formal proofs and extensive testing"

A **performance-focused, drop-in replacement for `rnsd`** — the Reticulum
daemon — delivered in every shape a Reticulum node needs to exist:

- a **daemon** on Linux/desktop/server,
- **embedded firmware** on microcontrollers (`no_std + alloc`),
- and **platform SDKs**: iOS/SwiftPM, Android/Maven, npm,

with **formal proofs** and **extensive differential testing** as first-class
deliverables, not afterthoughts.

The single most clarifying fact about the target: **`rnsd` itself is about
thirty lines of real code.** Strip the example-config string and the Python
daemon is `Reticulum(configdir=…)` followed by `while True: sleep(1)`.
Everything a node *does* lives inside `Reticulum` and the `Transport` engine it
starts. So "drop-in `rnsd`" means **be `Reticulum` + `Transport` + the
interfaces + the shared-instance socket** — it does **not** mean reimplement
all of RNS-the-library.

That distinction draws our scope line. `Link`, `Resource`, `Channel`, `Buffer`
are **application-facing** surfaces — an app opens a link and sends a resource;
the daemon only *transports the packets*. So a piece of library surface earns
its place in the codebase only if:

1. **daemon behavior needs it**, or
2. **a named SDK consumer ships it.**

If neither is true, it is reference material, not code we carry. When in doubt,
name the consumer out loud. If you can't, you have your answer.

---

## 2. The one idea: parity is a property of the *contract*, not the *implementation*

The natural instinct when porting any reference implementation is to
**transliterate** it: chop the reference's functions into your-language
functions, mirror its variable names and procedural shape, and pin that mirror
with tests. It feels safe. It is a trap.

Transliteration is how you end up with:

- a parameter named `tag` that is *actually* a 16-byte random dedup **nonce** —
  the name tells you nothing, and "is it a hash? what kind?" is the wrong
  question, because the honest answer is "none, it's random";
- a function that hands back a raw `Vec<u8>` with byte-juggling helpers
  scattered around it, instead of a typed value with one clean encode step;
- `build_x_into(&mut buf) -> usize` — a literal C idiom ("write into a buffer,
  return the count") wearing Rust's clothes;
- a module that is **10–20× the size** of the Python it mirrors, because every
  `if` branch became its own input/decision/outcome triple.

You wind up testing *your transliteration* against *their implementation*. That
is enormous, brittle, and — worst of all — gives false confidence: it *looks*
like Python, so surely it *acts* like Python. (It might not. You just have 20×
more surface for the difference to hide in.)

**You owe the reference fidelity at exactly two boundaries:**

1. **The wire.** The exact bytes on the air; the byte position of every field.
   Fully specifiable and *exhaustively* testable with byte vectors. There is
   **one** serialization boundary; everything above it is typed.
2. **The behavior.** Given a stream of inbound packets and a clock, which
   packets go out and what state persists.

**Between those two boundaries, the model is yours.** Clean, typed, idiomatic
Rust. It owes the reference *nothing* — not its names, not its function
breakdown, not its types.

### The mental model: a web server

A web server has a typed domain model and its own storage *internally*, and it
exposes a **wire contract** (its API schema) plus a **behavioral contract**
(endpoint semantics, statefulness, idempotency). You verify it with tests
*against that contract*. Nobody builds a web server by porting another server's
request handler line-by-line and checking the correspondence — that would be
insane.

The RNS daemon is the same shape:

| Web server | RNS daemon |
| --- | --- |
| API/wire schema | packet byte layouts |
| endpoint semantics + state | packets-in → packets-out + persisted state |
| a reference server | the Python implementation — an **executable oracle to diff against**, not a thing to mirror |
| your domain model + storage | your clean typed core, owing the reference nothing |

So you are **not** torn between "faithful to Python" and "clean Rust." They only
conflict if you demand faithfulness *everywhere*. Faithfulness lives **at the
two boundaries**; cleanliness lives **between them**.

---

## 3. The shape: a game engine

The core is a **pure tick**:

```
tick(&mut State, Input, dt) -> Effects
```

No I/O, no clock reading, no syscalls *inside* the core. **Time is an
argument.** Inbound is data. Outbound is data — typed `Effect`s that the
surrounding loop interprets, not calls the core makes itself.

This is not exotic. It is the same attractor that Elm/Redux, deterministic
lockstep netcode, ECS game loops, and embedded superloops all evolve toward
independently. A pure function of `(state, input, time)` is the most testable
and most portable object there is. **Arriving at this shape is a sign the
decomposition is natural, not forced.**

### The Host is the seam — and the impurity budget

The platform supplies a small **`Host`**: a clock, byte I/O, storage. The core
consumes it; a single generic run-loop drives the core against any `Host`.

The discipline this buys is mechanical, not aspirational: **the only way
platform-specificity can enter the system is through that trait.** So the
trait's surface *is* the complete, reviewable inventory of everything this stack
needs from the world. Wanting to add a method to it is a visible alarm that you
are about to leak the platform into the brain. Keep the trait small → the core
is *provably* pure. That is what force-functions nice predicates, typed
decisions, and clock-as-data — you don't have to remember to do it; the
architecture makes the wrong thing hard.

### One core, N tiny hosts

The Linux daemon, the ESP32 superloop, the Android foreground service, the
WASM/npm package — **none of them reimplement the protocol.** Each implements a
~5-method `Host` ("read from UART / give me millis / write these bytes / store
this blob") and runs the provided loop. The microcontroller doesn't get the
protocol brain; it gets the trait.

**The `Host` trait is the contract between "the protocol brain" and "this
particular body."** That is why every new target is cheap, and why the whole
multi-platform promise is one core plus a handful of small adapters.

---

## 4. How we know it's right

The Python implementation is an **executable oracle**, run side-by-side via
`reticulum-pyo3`. (The prior effort graded against `RNS 1.1.9`; the rebuild can
chase a newer version — the protocol shape is stable, so the bump is cheap.)

Because the core is a *pure function*, the differential test is both trivial and
strong:

- **Wire:** pin byte layouts with exhaustive byte vectors.
- **Behavior:** feed identical input streams to Python and to `tick`, and diff
  the `Effect` streams.

We **re-aim** the existing test machinery; we do not throw it away. The oracle
and the generator scripts are gold. Point them at the **contract boundaries**
instead of at internal-function mirrors. The result is *fewer, higher-level*
tests, *clean* internals, and *equal-or-better* assurance — because a behavioral
diff catches divergence regardless of how the internals are shaped.

Genuinely hard-to-test subtleties (timing, races) stop being scary the moment
time is an explicit injected input: "hard because it's real-time" becomes
"deterministic because the clock is an argument."

---

## 5. What "good" looks like in a single file

- **Invalid states are unrepresentable.** Errors are narrow, honest, and scoped
  to the operation that produces them. No stringly-typed errors.
- **Names say what a thing *is*,** not what the reference called it. A nonce is
  a nonce, not a `tag`. If you inherited a label from Python, you probably
  inherited the wrong abstraction with it.
- **Typed decisions over scattered booleans.** A decision is a value with a
  name, not a pile of `if`s returning `bool`.
- **One serialization boundary,** with guaranteed wire byte positions. Not
  ad-hoc byte juggling, and not `serde`-by-default (it is awkward on embedded
  and does not pin layout) — a small, total, `no_std`-friendly wire codec where
  each type owns its bytes.
- **Design Rust APIs, not transliterated C-isms.** If you're writing
  `_into(&mut buf) -> usize`, stop and ask what the *value* is.
- **If a doc comment just restates the name, rename the thing instead.**

---

## 6. The litmus test (run this on any file, without asking anyone)

1. Does this code **reach out** for the clock, a socket, or storage — or are
   time and I/O passed *in* as data? (Reaching out, outside a `Host` impl? Stop.)
2. Is every type's meaning **obvious from its name**, or am I carrying a label
   borrowed from the reference?
3. Could I **delete this and still pass the contract tests**? If yes, it's
   transliteration cruft.
4. Is there **exactly one place** where bytes ↔ types for this packet?
5. Does honoring this require a **new `Host` method**? If so, justify it out
   loud — it's a real cost.
6. Is this library surface justified by **daemon behavior or a named SDK
   consumer**? If neither, it's reference, not code we carry.

---

## What this is not (yet)

This is the ethos, not a spec. The concrete `State` / `Input` / `Effect` /
`Host` types, the module layout, and the wire-codec mechanism are a **separate,
later step** — and quite possibly that step is best delivered as **one worked
vertical slice of real code** (a single protocol behavior, end to end, in the
engine/host shape) rather than as another document. A reference implementation
teaches the shape; a spec document tends to rot. We'll decide that when we get
there.

> Port the contract, not the implementation. Build one pure engine, and let each
> platform bring a thin host.
