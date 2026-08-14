import { Tag } from "../casework.js";
import type {
  RuntimeRejected,
  UnsupportedInterface,
} from "./index.js";

export type RNodeConnectOutcome =
  | UnsupportedInterface<"rnode">
  | RuntimeRejected;

type RNodeRuntimeHost = {
  runtimeReadiness(): Tag<"Ready"> | RuntimeRejected;
};

export class RNodeInterface {
  readonly name = "rnode" as const;
  readonly #host: RNodeRuntimeHost;

  constructor(host: RNodeRuntimeHost) {
    this.#host = host;
  }

  async connect(): Promise<RNodeConnectOutcome> {
    const ready = this.#host.runtimeReadiness();
    if (ready.tag !== "Ready") {
      return ready;
    }
    return Tag("UnsupportedInterface", {
      interface: "rnode",
      host: "Browser",
    });
  }
}
