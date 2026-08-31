import { getContext, setContext } from "svelte";
import { readable } from "svelte/store";
import type { Readable } from "svelte/store";
import type { Prns } from "../index.js";
import type {
  PrnsProjectionValue,
  PrnsView,
} from "../projections.js";
import {
  PrnsProviderMissingError,
  requireClientBoundary,
} from "./client.js";

const prnsKey = Symbol("personal-rns/svelte");

export function setPrnsContext(prns: Prns): Prns {
  requireClientBoundary("personal-rns/svelte");
  return setContext(prnsKey, prns);
}

export function getPrnsContext(): Prns {
  requireClientBoundary("personal-rns/svelte");
  const prns = getContext<Prns | undefined>(prnsKey);
  if (prns === undefined) {
    throw new PrnsProviderMissingError("personal-rns/svelte");
  }
  return prns;
}

export function prnsReadable<View extends PrnsView>(
  view: View,
): Readable<PrnsProjectionValue<View>> {
  const projection = getPrnsContext().projection(view);
  return readable(projection.latest().value, (set) =>
    projection.subscribe(() => set(projection.latest().value))
  );
}

export {
  PrnsClientBoundaryRequiredError,
  PrnsProviderMissingError,
} from "./client.js";
