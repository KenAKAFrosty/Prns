# Building the Node addon

## Prerequisites

- Rust via [rustup](https://rustup.rs) (stable; the repo floor is 1.90)
- Node.js >= 20 with npm
- macOS only: Xcode Command Line Tools (`xcode-select --install`)

## Build and test

```
cd prns-napi
npm ci
npm run build:debug
npm test
```

Expect every test to pass. The suite starts real nodes over localhost TCP, so allow the network prompt if macOS asks.

## Release artifacts

Each target drops a `personal-rns.<platform>.node` beside `package.json`.

On an Apple Silicon Mac, both darwin targets:

```
rustup target add x86_64-apple-darwin
npm run build
npm run build -- --target x86_64-apple-darwin
```

This produces `personal-rns.darwin-arm64.node` and `personal-rns.darwin-x64.node`.

## Publishing

Do not publish platform packages from personal npm accounts — the first publish of a package name claims its ownership. Send the built `.node` files (plus your `npm test` output) to the release owner, who assembles and publishes them with `napi create-npm-dirs` and `npm publish` from the release account.
