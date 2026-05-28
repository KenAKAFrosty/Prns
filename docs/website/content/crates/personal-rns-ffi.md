## When to reach for this

Use this when your app isn't written in Rust but you still want
Reticulum on the inside.

- An Android app in Kotlin.
- An iOS or macOS app in Swift.
- A Python service or notebook.

You get the same engine your Rust friends are using — no
re-implementation, no protocol drift, no second copy to keep in
sync.

## What you get

One [uniffi](https://mozilla.github.io/uniffi-rs/) interface
description compiles into three shipping artifacts:

| Platform | Artifact |
|----------|----------|
| Android  | `.aar` (Kotlin) |
| iOS / macOS | `.xcframework` (Swift) |
| Python   | `.whl` (PyPI wheel) |

All three load the same compiled engine. Add a Reticulum function?
You write it once, in Rust, and all three languages get it next
build.

## Status

The bindings layer is functional but minimal — enough surface to
drive the engine through the same `step` loop the daemon uses.
Higher-level helpers (identity management, LXMF inbox, address book)
will grow on top of this once the corresponding engine layers land.
