import { Tag, match } from "../../dist/casework.js";
import initPortableWasm, {
  profileEd25519Sign,
  profileEd25519Vector,
  profileX25519,
  profileX25519Vector,
} from "/prns-wasm/smoke/pkg/prns_wasm.js";

const ready = initPortableWasm();

void ready.then(() => {
  self.postMessage(Tag("Ready", {
    ed25519Vector: profileEd25519Vector(),
    x25519Vector: profileX25519Vector(),
  }));
});

self.addEventListener("message", ({ data }) => {
  void ready.then(() => {
    match(data, {
      Run: ({ id, operation, iterations }) => {
        try {
          const checksum = operation === "Ed25519Sign"
            ? profileEd25519Sign(iterations)
            : profileX25519(iterations);
          self.postMessage(Tag("Completed", { id, checksum }));
        } catch (error) {
          self.postMessage(Tag("Failed", {
            id,
            detail: String(error?.stack ?? error),
          }));
        }
      },
    });
  });
});
