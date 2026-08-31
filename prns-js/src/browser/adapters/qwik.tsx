import {
  component$,
  createContextId,
  noSerialize,
  useContext,
  useContextProvider,
  useSignal,
  useVisibleTask$,
} from "@builder.io/qwik";
import type {
  NoSerialize,
  ReadonlySignal,
  Signal,
  JSXChildren,
} from "@builder.io/qwik";
import { prnsView } from "personal-rns/browser";
import type {
  Prns,
  PrnsProjection,
  PrnsProjectionValue,
  PrnsView,
} from "personal-rns/browser";
import {
  PrnsClientBoundaryRequiredError,
  PrnsProviderMissingError,
} from "./client.js";

const PrnsContext = createContextId<NoSerialize<Prns>>(
  "personal-rns/qwik",
);

export type PrnsProviderProps = {
  readonly prns: NoSerialize<Prns>;
  readonly children?: JSXChildren;
};

export const PrnsProvider = component$<PrnsProviderProps>((props) => {
  requireClient();
  if (props.prns === undefined) {
    throw new PrnsProviderMissingError("personal-rns/qwik");
  }
  useContextProvider(PrnsContext, props.prns);
  return <>{props.children}</>;
});

export function usePrns(): Prns {
  requireClient();
  const prns = useContext(PrnsContext);
  if (prns === undefined) {
    throw new PrnsProviderMissingError("personal-rns/qwik");
  }
  return prns;
}

export function usePrnsProjection<View extends PrnsView>(
  view: View,
): ReadonlySignal<PrnsProjectionValue<View>> {
  const projection = usePrns().projection(view);
  const retained = useSignal<NoSerialize<PrnsProjection<PrnsProjectionValue<View>>>>(
    noSerialize(projection),
  );
  const current: Signal<PrnsProjectionValue<View>> = useSignal(
    projection.latest().value,
  );
  useVisibleTask$(({ cleanup }) => {
    const active = retained.value;
    if (active === undefined) {
      throw new PrnsProviderMissingError("personal-rns/qwik");
    }
    current.value = active.latest().value;
    cleanup(active.subscribe(() => {
      current.value = active.latest().value;
    }));
  }, { strategy: "document-ready" });
  return current;
}

export function diagnosticsView(maximumEvents: number): PrnsView {
  return prnsView("Diagnostics", { maximumEvents });
}

function requireClient(): void {
  if (typeof globalThis.window === "undefined") {
    throw new PrnsClientBoundaryRequiredError("personal-rns/qwik");
  }
}

export {
  PrnsClientBoundaryRequiredError,
  PrnsProviderMissingError,
};
