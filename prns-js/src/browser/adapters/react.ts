import {
  createContext,
  createElement,
  useContext,
  useSyncExternalStore,
} from "react";
import type { ReactNode } from "react";
import type { Prns } from "../index.js";
import type {
  PrnsProjectionValue,
  PrnsView,
} from "../projections.js";
import {
  PrnsProviderMissingError,
  requireClientBoundary,
} from "./client.js";

const PrnsContext = createContext<Prns | undefined>(undefined);

export type PrnsProviderProps = {
  readonly prns: Prns;
  readonly children?: ReactNode;
};

export function PrnsProvider(props: PrnsProviderProps): ReactNode {
  requireClientBoundary("personal-rns/react");
  return createElement(
    PrnsContext.Provider,
    { value: props.prns },
    props.children,
  );
}

export function usePrns(): Prns {
  requireClientBoundary("personal-rns/react");
  const prns = useContext(PrnsContext);
  if (prns === undefined) {
    throw new PrnsProviderMissingError("personal-rns/react");
  }
  return prns;
}

export function usePrnsProjection<View extends PrnsView>(
  view: View,
): PrnsProjectionValue<View> {
  const projection = usePrns().projection(view);
  return useSyncExternalStore(
    (changed) => projection.subscribe(changed),
    () => projection.latest().value,
  );
}

export {
  PrnsClientBoundaryRequiredError,
  PrnsProviderMissingError,
} from "./client.js";
