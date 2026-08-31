import { Tag } from "../../casework.js";
import type { Tag as Tagged } from "../../casework.js";
import type {
  CommandSettlementFor,
  HostCommand,
  InterfaceSnapshot,
  LifecycleState,
  RouteSnapshot,
} from "../../contract.js";
import type { PrnsDiagnosticEvent } from "../events.js";
import type { Prns } from "../index.js";
import { prnsView } from "../projections.js";
import type {
  ActiveLinkSnapshot,
  PrnsProjection,
} from "../projections.js";
import {
  PrnsClientBoundaryRequiredError,
  requireClientBoundary,
} from "./client.js";

export const PRNS_BRIDGE_ELEMENT_NAME = "prns-bridge";
export const PRNS_BRIDGE_CHANGE_EVENT = "prns-change";

export type PrnsBridgeConfiguration = {
  readonly prns: Prns;
  readonly diagnosticMaximumEvents: number;
};

export type PrnsBridgeState = {
  readonly version: number;
  readonly lifecycle: LifecycleState;
  readonly interfaces: readonly InterfaceSnapshot[];
  readonly routes: readonly RouteSnapshot[];
  readonly links: readonly ActiveLinkSnapshot[];
  readonly diagnostics: readonly PrnsDiagnosticEvent[];
};

export type PrnsBridgeSnapshot =
  | Tagged<"Ready", PrnsBridgeState>
  | Tagged<"Unconfigured">;

export type DefinePrnsBridgeOutcome =
  | Tagged<"Defined">
  | Tagged<"AlreadyDefined">;

export class PrnsBridgeUnconfiguredError extends Error {
  constructor() {
    super("prns-bridge must be configured before executing commands");
    this.name = "PrnsBridgeUnconfiguredError";
  }
}

export interface PrnsBridgeElement extends HTMLElement {
  configure(configuration: PrnsBridgeConfiguration): void;
  snapshot(): PrnsBridgeSnapshot;
  execute<Command extends HostCommand>(
    command: Command,
  ): Promise<CommandSettlementFor<Command>>;
}

export function definePrnsBridgeElement(
  registry: CustomElementRegistry = globalThis.customElements,
): DefinePrnsBridgeOutcome {
  requireClientBoundary("personal-rns/web-component");
  if (registry.get(PRNS_BRIDGE_ELEMENT_NAME) !== undefined) {
    return Tag("AlreadyDefined");
  }
  class BrowserPrnsBridgeElement extends HTMLElement implements PrnsBridgeElement {
    #configuration: PrnsBridgeConfiguration | undefined;
    #state: PrnsBridgeState | undefined;
    #release: (() => void)[] = [];
    #scheduled = false;
    #version = 0;

    configure(configuration: PrnsBridgeConfiguration): void {
      this.#unbind();
      this.#configuration = configuration;
      this.#state = this.#readState();
      if (this.isConnected) {
        this.#bind();
        this.#schedule();
      }
    }

    snapshot(): PrnsBridgeSnapshot {
      return this.#state === undefined
        ? Tag("Unconfigured")
        : Tag("Ready", this.#state);
    }

    execute<Command extends HostCommand>(
      command: Command,
    ): Promise<CommandSettlementFor<Command>> {
      if (this.#configuration === undefined) {
        return Promise.reject(new PrnsBridgeUnconfiguredError());
      }
      return this.#configuration.prns.execute(command);
    }

    connectedCallback(): void {
      if (this.#configuration !== undefined) {
        this.#bind();
        this.#schedule();
      }
    }

    disconnectedCallback(): void {
      this.#unbind();
    }

    #bind(): void {
      this.#unbind();
      const projections = this.#projections();
      for (const projection of projections) {
        this.#release.push(projection.subscribe(() => this.#schedule()));
      }
    }

    #unbind(): void {
      for (const release of this.#release.splice(0)) {
        release();
      }
    }

    #schedule(): void {
      if (this.#scheduled) {
        return;
      }
      this.#scheduled = true;
      queueMicrotask(() => {
        this.#scheduled = false;
        if (!this.isConnected || this.#configuration === undefined) {
          return;
        }
        this.#state = this.#readState();
        this.dispatchEvent(new CustomEvent<PrnsBridgeState>(
          PRNS_BRIDGE_CHANGE_EVENT,
          { detail: this.#state, bubbles: true, composed: true },
        ));
      });
    }

    #readState(): PrnsBridgeState {
      const [lifecycle, interfaces, routes, links, diagnostics] =
        this.#projections();
      this.#version += 1;
      return Object.freeze({
        version: this.#version,
        lifecycle: lifecycle.latest().value,
        interfaces: interfaces.latest().value,
        routes: routes.latest().value,
        links: links.latest().value,
        diagnostics: diagnostics.latest().value,
      });
    }

    #projections(): [
      PrnsProjection<LifecycleState>,
      PrnsProjection<readonly InterfaceSnapshot[]>,
      PrnsProjection<readonly RouteSnapshot[]>,
      PrnsProjection<readonly ActiveLinkSnapshot[]>,
      PrnsProjection<readonly PrnsDiagnosticEvent[]>,
    ] {
      const configuration = this.#configuration;
      if (configuration === undefined) {
        throw new PrnsBridgeUnconfiguredError();
      }
      const prns = configuration.prns;
      return [
        prns.projection(prnsView("Lifecycle")),
        prns.projection(prnsView("Interfaces")),
        prns.projection(prnsView("Routes")),
        prns.projection(prnsView("Links")),
        prns.projection(prnsView("Diagnostics", {
          maximumEvents: configuration.diagnosticMaximumEvents,
        })),
      ];
    }
  }
  registry.define(PRNS_BRIDGE_ELEMENT_NAME, BrowserPrnsBridgeElement);
  return Tag("Defined");
}

export { PrnsClientBoundaryRequiredError } from "./client.js";

declare global {
  interface HTMLElementTagNameMap {
    "prns-bridge": PrnsBridgeElement;
  }
}
