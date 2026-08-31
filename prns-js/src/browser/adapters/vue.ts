import {
  getCurrentScope,
  inject,
  onScopeDispose,
  provide,
  readonly,
  shallowRef,
} from "vue";
import type { InjectionKey, ShallowRef } from "vue";
import type { Prns } from "../index.js";
import type {
  PrnsProjectionValue,
  PrnsView,
} from "../projections.js";
import {
  PrnsProviderMissingError,
  requireClientBoundary,
} from "./client.js";

const prnsKey: InjectionKey<Prns> = Symbol("personal-rns/vue");

export class PrnsVueScopeRequiredError extends Error {
  constructor() {
    super("personal-rns/vue projections require an active Vue effect scope");
    this.name = "PrnsVueScopeRequiredError";
  }
}

export function providePrns(prns: Prns): void {
  requireClientBoundary("personal-rns/vue");
  provide(prnsKey, prns);
}

export function usePrns(): Prns {
  requireClientBoundary("personal-rns/vue");
  const prns = inject(prnsKey);
  if (prns === undefined) {
    throw new PrnsProviderMissingError("personal-rns/vue");
  }
  return prns;
}

export function usePrnsProjection<View extends PrnsView>(
  view: View,
): Readonly<ShallowRef<PrnsProjectionValue<View>>> {
  if (getCurrentScope() === undefined) {
    throw new PrnsVueScopeRequiredError();
  }
  const projection = usePrns().projection(view);
  const current = shallowRef(projection.latest().value) as ShallowRef<PrnsProjectionValue<View>>;
  const release = projection.subscribe(() => {
    current.value = projection.latest().value;
  });
  onScopeDispose(release);
  return readonly(current) as Readonly<ShallowRef<PrnsProjectionValue<View>>>;
}

export {
  PrnsClientBoundaryRequiredError,
  PrnsProviderMissingError,
} from "./client.js";
