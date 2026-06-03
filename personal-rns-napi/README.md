# @personal/rns

TypeScript and Node.js bindings for the Personal Reticulum runtime.

This package is a local scaffold while the SDK surface settles. It uses
`napi-rs` to expose the Rust runtime as a Node-API native addon.

```ts
import { ReticulumRuntime, version } from "@personal/rns";

const runtime = new ReticulumRuntime();

console.log(version());
console.log(runtime.tickCount());
console.log(runtime.tick());
console.log(runtime.tickCount());
```
