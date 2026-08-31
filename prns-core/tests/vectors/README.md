# Pinned cryptographic test vectors

The Wycheproof corpora in this directory are copied verbatim from
[`C2SP/wycheproof`](https://github.com/C2SP/wycheproof) commit
`dac1dd4729fd1f8dd9e1e9f3dce51d783da6c166`:

- `testvectors_v1/ed25519_test.json`
- `testvectors_v1/x25519_test.json`

They are checked in so ordinary tests are deterministic and never require the network. Upstream
licenses Wycheproof under Apache-2.0; its
[`LICENSE`](https://github.com/C2SP/wycheproof/blob/dac1dd4729fd1f8dd9e1e9f3dce51d783da6c166/LICENSE)
and [`CONTRIBUTORS`](https://github.com/C2SP/wycheproof/blob/dac1dd4729fd1f8dd9e1e9f3dce51d783da6c166/CONTRIBUTORS)
apply to these fixtures.

SHA-256:

```text
752d2ea7d7c6cf4736381b6cbacb61f8182b126ab7cd9b058f00c50084975536  wycheproof_ed25519_dac1dd47.json
35c3f5231cf25cc640b524d403461deee9e49441d5d915a3a25b2c8ff5adbe7d  wycheproof_x25519_dac1dd47.json
```

When updating the pin, update the filenames, hashes, asserted corpus sizes/result counts, and the
source commit together. Review upstream vector changes before accepting new expected behavior.
