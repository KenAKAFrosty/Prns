# personal-rns

`personal-rns` provides one casework-shaped JavaScript API for native Node.js, Bun, and browsers.

The root export selects the native backend in Node.js and Bun and the cooperative WebAssembly backend in browser bundlers. Explicit `personal-rns/native` and `personal-rns/browser` subpaths are available when runtime selection must be fixed.

Application events and diagnostics are separate single-consumer async iterables. Commands settle their returned promises. Public binary values are `Uint8Array` instances with semantic brands in TypeScript.
