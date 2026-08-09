# Contributing

Thank you for considering a contribution to Prns. We welcome open collaboration, and appreciate quality help. 

This is the repo-wide contribution guide for both human and automated contributors.

## What we value

**Ownership.** Contributors own their submissions. 
AI tools (and other assistance like pair programming, web snippets, etc.) are welcome, but slop is slop regardless of how it was produced. 
How you wield your tools is still under your control, and what you submit is ***yours***.

**Stewardship.** Be a good guest on the device and a good neighbor on the
spectrum. Conserve battery, respect shared mediums, and avoid taking more than
you need. And when you can, leave code better than you found it.

**Craft.** The inside should be as carefully made as the outside feels.
Self-documenting code, exhaustive reasoning, no shortcuts that shift cost to
the next reader.

**Inquiry.** Treat the work as science: form a hypothesis, measure the
outcome, share the result. Findings belong in the open where peers can
reproduce, refute, or build on them. Claims earn their keep through
corroborated evidence.

## Design Principles
- Correct. Robust. Fast. 
- API design is paramount
- Make invalid states unrepresentable
- Names are extremely important
- Encode principles structurally
- Maintain a single source of truth. Each fact should have one authoritative owner; all other representations derive from it.
- Comments should be an *exception*, not a rule. Many comments are papering over a bad name or bad API.
- Overall our style is proudly FP-flavored, but with deviations for practical or performance reasons.

## Rust
- Newtypes, named enums, all incredibly powerful. We should not be shuffling around opaque bytes, or strings, or loose numbers, any more than we need to (like when the values being opaque *is the point*; that's a different thing)
- Do not allow "Stringly-typed" errors. Use an enum.
- Do not use `anyhow` in repo code. Prefer typed error enums with `thiserror`
  or an explicit domain error type so callers can preserve structure.
- Be very careful using `Option`s. They are a sensible choice when the semantics are "Either a thing, or some only-one-variant failure; especially when that outcome is not fair to be described as a 'failure' (e.g, a lookup in a key-value data structure)" . In most cases, a proper Result or just custom outcome enum is a far superior choice and should be preferred.
- Don't blindly add derived traits like `Copy`. Assume no derived traits, and only add them once they're truly needed.
- Avoid nesting as much as possible. Prefer early returns, which also pair very well with the aforementioned newtypes and named enums
- Prefer self-documenting APIs. Avoid callsites like `foo(false)` or `bar(None)`
  when an enum, named method, newtype, or clearer parameter shape would make
  intent obvious.
- Avoid loose string literals and magic numbers. Use simple CONSTs to give them semantic meaning and a clear usage graph
- Do not hide semantically required configuration behind permissive defaults or
  sentinel values. If a field is required for correctness, policy, or safety,
  make callers provide it explicitly instead of seeding placeholders like `0`,
  `None`, `u32::MAX`, empty strings, or broad "allow anything" enums and
  relying on follow-up setters.
- Prefer construction shapes that make omission a compile-time error:
  field-named struct literals, constructors that take the full
  invariant-bearing input, enums/newtypes, or staged builders that only
  default values that are truly optional.
- Prefer exhaustive `match` statements when practical. Avoid wildcard arms when
  they hide meaningful cases.
- Prefer private modules and an explicitly curated public API.
- Keep callsites readable. Avoid opaque positional literals unless the
  surrounding code makes the meaning obvious.
- If using `format!`, inline variables into `{}` when that keeps the code
  clear.

## Structure

- Place by ownership. Give every concept one authoritative home in the
  narrowest layer that owns it. Decide that ownership before adding modules;
  adapters and presentations should depend inward on it.
- Make paths communicate meaning. A reader should be able to infer a file's
  domain, owner, and abstraction level from its path. The module tree should
  mirror the conceptual tree; avoid vague root modules and mismatched siblings.
- Represent modules consistently. Use `foo.rs` for a leaf module and
  `foo/mod.rs` when `foo` owns child modules or separately housed tests.
  Directories should represent real namespaces, not storage conveniences.
- Comments explain durable local invariants or constraints, not code narration,
  subsystem tours, editorial history, or implementation restatement.
- Comment prose uses natural paragraphs and punctuation, allowing the viewer to
  wrap lines; em dashes are reserved for cases where they are genuinely the
  clearest grammar.
- Prefer adding a new module over growing an already-large module.
- Avoid introducing small helper functions that are only used once unless they
  clearly improve readability.
- Keep related tests and documentation close to the code that owns the
  behavior.
- If a central crate or module is already overloaded, prefer extracting code
  into a more focused crate or module instead of adding more to the hotspot.

## Tests

- Run the most targeted tests for the crate or module you changed before
  broader test runs.
- Prefer assertions on whole values over field-by-field assertions when
  practical.
- Avoid mutating process-global environment in tests when dependencies can be
  passed explicitly.
- Treat mutation testing as diagnostic evidence that requires human triage, not
  as a score or a reason by itself to rewrite production code. Prefer a stronger
  behavioral test when a mutant exposes a real gap.
- Require an independent correctness or design rationale for every production
  change prompted by mutation analysis. Performance-shaped code also requires
  comparative measurement against the exact prior implementation.
- Follow the repository's [verification ladder](docs/testing.md); report the
  exact commands and host coverage instead of implying unrun platforms passed.

## Repository operations

The repository has two deliberate control planes:

- `./tools/prns` (on Windows: `.\tools\prns.cmd`) discovers and runs supported
  build, guide, release, device, and repository operations. Start with
  `./tools/prns doctor getting-started`, list tasks with `./tools/prns list`,
  and explain one with `./tools/prns explain TASK_ID`.
- `python3 validation/run.py` (`python` on Windows) discovers and runs tests,
  proofs, interoperability checks, and release-readiness evidence. Use
  `--platform current` for portable plus current-host suites.

Implementation files below `tools/` are internal. CI and documentation invoke
named `./tools/prns` tasks so paths can be reorganized without creating another
public script surface. Enable the repository Git hook once per clone with
`git config core.hooksPath .githooks`.


## License of contributions

By contributing, you agree that your contributions will be dual licensed under the MIT and Apache-2.0 licenses, matching the project's [license](README.md#license).
