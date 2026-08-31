import {
  createComponent,
  createContext,
  from,
  useContext,
} from "solid-js";
import type { Accessor, JSX } from "solid-js";
import type { Prns } from "../index.js";
import type {
  PrnsProjectionValue,
  PrnsView,
} from "../projections.js";
import {
  PrnsProviderMissingError,
  requireClientBoundary,
} from "./client.js";

const PrnsContext = createContext<Prns>();

export type PrnsProviderProps = {
  readonly prns: Prns;
  readonly children?: JSX.Element;
};

export function PrnsProvider(props: PrnsProviderProps): JSX.Element {
  requireClientBoundary("personal-rns/solid");
  return createComponent(PrnsContext.Provider, {
    value: props.prns,
    get children() {
      return props.children;
    },
  });
}

export function usePrns(): Prns {
  requireClientBoundary("personal-rns/solid");
  const prns = useContext(PrnsContext);
  if (prns === undefined) {
    throw new PrnsProviderMissingError("personal-rns/solid");
  }
  return prns;
}

export function createPrnsProjection<View extends PrnsView>(
  view: View,
): Accessor<PrnsProjectionValue<View>> {
  const projection = usePrns().projection(view);
  return from(
    (set) => projection.subscribe(() => set(() => projection.latest().value)),
    projection.latest().value,
  );
}

export {
  PrnsClientBoundaryRequiredError,
  PrnsProviderMissingError,
} from "./client.js";
