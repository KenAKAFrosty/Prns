jest.mock("@react-native-async-storage/async-storage", () =>
  jest.requireActual("@react-native-async-storage/async-storage/jest/async-storage-mock"),
);

// The bundled Hermes runtime does not provide `toSorted`, while the newer Node.js
// used by Jest does. Mask both collection prototypes so native-only crashes cannot
// hide behind the test runtime.
for (const prototype of [Array.prototype, Object.getPrototypeOf(Uint8Array.prototype)]) {
  Object.defineProperty(prototype, "toSorted", {
    configurable: true,
    value: undefined,
    writable: true,
  });
}
