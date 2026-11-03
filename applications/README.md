# PRNS Chat native product

The [`prns-chat-app`](prns/chat-app/src/lib.rs) crate selects one service image
for PRNS Chat. It owns installation bootstrap, private storage paths, resource
budgets and separate controller and management-target keys. Identity, contact,
group and host operations remain generated from their canonical crates.

The product retains the service-runtime owner. Local readiness allows saved
contacts and history to remain readable independently of network readiness.
Explicit shutdown drains accepted work before releasing the installation.
The general [service runtime](../prns-service-runtime/README.md) and
[mobile SDK](../prns-react-native/README.md) remain usable without this product.

Run the native product checks from the repository root:

```sh
cargo test --locked --manifest-path applications/Cargo.toml --workspace --all-targets --all-features
cargo clippy --locked --manifest-path applications/Cargo.toml --workspace --all-targets --all-features -- -D warnings
```
