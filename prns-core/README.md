# Personal RNS (Prns)

This crate is one package in the Personal RNS public Rust graph. Quick overviews, the complete feature guide, API documentation, examples, and the cross-language SDK overview are available at [prns.dev](https://prns.dev) or [reticulum.rs](https://reticulum.rs), and in the [source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual MIT/Apache-2.0 license.

## Portable host capabilities

`prns-core::capabilities` defines optional, platform-neutral observations that applications and
future PRNS policy can share without depending on a particular board or operating system. The
initial capability set includes validated fixed-point geographic positions, an allocation-free
GNSS/NMEA provider, and coherent battery/external-power observations. Embedded, mobile, and desktop hosts remain
responsible for acquiring the observations, and core never publishes them or changes network
behavior merely because they are available.
