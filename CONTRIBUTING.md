# Contributing

Thanks for your interest in Prns. We welcome open collaboration and appreciate quality help. 

This is the repo-wide contribution guide for both human and automated contributors.

## What we value

**Ownership.** Contributors own their submissions. AI tools (and other assistance like pair
programming, web snippets, etc.) are welcome, but slop is slop regardless of
how it was produced. How you wield your tools is still under your control,
and what you submit is ***yours***.

**Stewardship.** Be a good guest on the device and a good neighbor on the
spectrum. Conserve battery, respect shared mediums, and avoid taking more than
you need.

**Craft.** The inside should be as carefully made as the outside feels.
Self-documenting code, exhaustive reasoning, no shortcuts that shift cost to
the next reader.

**Inquiry.** Treat the work as science: form a hypothesis, measure the
outcome, share the result. Findings belong in the open where peers can
reproduce, refute, or build on them. Claims earn their keep through
corroborated evidence.


## Rust

- Prefer self-documenting APIs. Avoid callsites like `foo(false)` or `bar(None)`
  when an enum, named method, newtype, or clearer parameter shape would make
  intent obvious.
- Do not hide semantically required configuration behind permissive defaults or
  sentinel values. If a field is required for correctness, policy, or safety,
  make callers provide it explicitly instead of seeding placeholders like `0`,
  `None`, `u32::MAX`, empty strings, or broad "allow anything" enums and
  relying on follow-up setters.
- Prefer construction shapes that make omission a compile-time error:
  field-named struct literals, constructors that take the full
  invariant-bearing input, enums/newtypes, or staged builders that only
  default values that are truly optional.
- Do not use `anyhow` in repo code. Prefer typed error enums with `thiserror`
  or an explicit domain error type so callers can preserve structure.
- Prefer exhaustive `match` statements when practical. Avoid wildcard arms when
  they hide meaningful cases.
- Prefer private modules and an explicitly curated public API.
- Keep callsites readable. Avoid opaque positional literals unless the
  surrounding code makes the meaning obvious.
- If using `format!`, inline variables into `{}` when that keeps the code
  clear.
- Do not allow "Stringly-typed" errors. Rust's enums are incredibly powerful. Use them.

## Structure

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
- Use diff-friendly assertions if the repo already has a preferred assertion
  crate or pattern.
- Avoid mutating process-global environment in tests when dependencies can be
  passed explicitly.


## License of contributions

By contributing, you agree that your contributions will be dual licensed under the MIT and Apache-2.0 licenses, matching the project's [license](README.md#license).
