import type { DevelopmentRuntime } from "@prns-internal/expo";
import { createContext, type PropsWithChildren, useContext, useMemo } from "react";

import { runtimeProvider } from "@/native/runtime-provider";
import type { RuntimeProvider } from "./runtime-provider.types";

export type ContactRuntimeView = {
  readonly availability: RuntimeProvider["availability"];
  readonly runtime: DevelopmentRuntime | null;
};

export type ContactRuntimeProviderSource =
  | {
      readonly availability: Extract<
        RuntimeProvider["availability"],
        { readonly type: "available" }
      >;
      readonly runtime: DevelopmentRuntime;
    }
  | {
      readonly availability: Extract<
        RuntimeProvider["availability"],
        { readonly type: "unavailable" }
      >;
    };

const ContactRuntimeContext = createContext<ContactRuntimeView | null>(null);

export function ContactRuntimeProvider({
  children,
  provider,
}: PropsWithChildren<{ readonly provider?: ContactRuntimeProviderSource }>) {
  const selectedProvider = provider ?? runtimeProvider;
  const value = useMemo<ContactRuntimeView>(
    () => ({
      availability: selectedProvider.availability,
      runtime: "runtime" in selectedProvider ? selectedProvider.runtime : null,
    }),
    [selectedProvider],
  );
  return <ContactRuntimeContext.Provider value={value}>{children}</ContactRuntimeContext.Provider>;
}

export function useContactRuntime(): ContactRuntimeView {
  const value = useContext(ContactRuntimeContext);
  if (value === null) {
    throw new Error("useContactRuntime must be used inside ContactRuntimeProvider");
  }
  return value;
}
