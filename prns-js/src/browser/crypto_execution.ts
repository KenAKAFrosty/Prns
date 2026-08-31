import type { Tag } from "../casework.js";

export type CryptoExecution =
  | Tag<"PortableWasm">
  | Tag<"WebCrypto">
  | Tag<"ParallelWorkers">;
