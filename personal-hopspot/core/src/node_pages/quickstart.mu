`c
`F6eb`!Prns quickstart`!`f

Small, real paths from a source clone to a running node.
`a

>>`!Run and inspect a node`!

From the repository root, keep the walkthrough separate from your normal Reticulum state:

`F999./tools/prns doctor node`f
`F999cargo prnsd --debug --detach -- --config target/quickstart-node`f
`F999cargo prnsd status --config target/quickstart-node`f
`F999cargo prnsd stop`f

The full prnsd README also covers interface inspection, logs, reattachment, and configuration.

>>`!Build with Rust`!

`F999cargo tools guide rust`f

This checked example creates two fresh nodes on localhost. Node A announces over an explicit TCP server; Node B receives the real Reticulum announce over its TCP client and exits.

>>`!Build for embedded`!

The embedded path is the same node-recipe API bound to real hardware, fixed storage, hardware entropy, and concrete interfaces — not a desktop-only API with an unrelated firmware shortcut.

The smallest board-backed starting point is the XIAO ESP32-C6 Hopspot:

`F999cd personal-hopspot/embedded/esp32`f
`F999cargo c6 --locked`f

Its entrypoint calls the shared C6 firmware recipe, which attaches USB, ESP-NOW, and Bluetooth. Read docs/embedded.md and personal-hopspot/README.md in the clone before flashing hardware.

>>`!Test and measure`!

`F999cargo test --locked`f
`F999cargo benchmark --smoke`f

The first is the normal core test path. The second checks the benchmark machinery without making a publishable performance claim.

>>`!Read the full guides`!

Everything essential is in the source clone. Render the canonical Markdown locally with:

`F999cargo run -p docs`f

That site includes getting-started, daemon, Rust, embedded, testing, and benchmark guides. Internet access is not required after dependencies are available.

`[Back to this node`:/page/index.mu]
