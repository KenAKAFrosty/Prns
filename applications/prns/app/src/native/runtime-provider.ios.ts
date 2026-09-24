import {
  bluetoothAuthorizationRuntime,
  developmentRuntime,
  scopedDevelopmentRuntime,
} from "@prns-internal/expo";
import type { RuntimeProvider } from "./runtime-provider.types";

export const runtimeProvider: RuntimeProvider = {
  availability: { type: "available", platform: "ios" },
  bluetoothAuthorization: bluetoothAuthorizationRuntime,
  runtime: developmentRuntime,
  acquire: (options) =>
    scopedDevelopmentRuntime(developmentRuntime, {
      ...options,
      // Native process launch owns the compiled iOS test peer, before JavaScript.
      nativeLifetime: "process",
    }),
};
