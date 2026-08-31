export type PrnsAdapterName =
  | "personal-rns/qwik"
  | "personal-rns/react"
  | "personal-rns/solid"
  | "personal-rns/svelte"
  | "personal-rns/vue"
  | "personal-rns/web-component";

export class PrnsClientBoundaryRequiredError extends Error {
  constructor(adapter: PrnsAdapterName) {
    super(`${adapter} requires a client-rendered browser boundary with a ready Prns instance`);
    this.name = "PrnsClientBoundaryRequiredError";
  }
}

export class PrnsProviderMissingError extends Error {
  constructor(adapter: PrnsAdapterName) {
    super(`${adapter} requires a Prns provider above the current consumer`);
    this.name = "PrnsProviderMissingError";
  }
}

export function requireClientBoundary(adapter: PrnsAdapterName): void {
  if (typeof globalThis.window === "undefined") {
    throw new PrnsClientBoundaryRequiredError(adapter);
  }
}
