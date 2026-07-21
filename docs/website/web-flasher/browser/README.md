# Guided flasher browser qualification

This suite runs the Dioxus flasher against a deterministic, signed four-board
candidate and an injected fake device bridge. The candidate uses a public
test-only Minisign key. Its private key is not stored in this repository.

`browser-test-fixture` changes only the compile-time trust root: channel and
manifest signatures, manifest semantics, artifact sizes, and SHA-256 values are
still verified. It cannot compile with `embedded-site`, is not a default
feature, and is rejected from production build commands and output by the
production-boundary gate.

Install the pinned Chromium revision once, then run the suites:

```text
npm run install:browser
npm run test:browser
npm run test:production-boundary
```

The browser suite requires no physical hardware. It records UI behavior around
the fake bridge; the lower-level bridge suite remains responsible for serial
protocol, MD5, and disconnect behavior.
