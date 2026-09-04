import {
  accessorySetupRuntime,
  developmentRuntime,
  scopedDevelopmentRuntime,
} from "@prns-internal/expo";
import type { RuntimeProvider } from "./runtime-provider.types";

export const runtimeProvider: RuntimeProvider = {
  availability: { type: "available", platform: "ios" },
  accessorySetup: accessorySetupRuntime,
  runtime: developmentRuntime,
  acquire: (options) =>
    scopedDevelopmentRuntime(developmentRuntime, {
      ...options,
      developmentTcpTarget: __DEV__ ? (process.env.EXPO_PUBLIC_PRNS_LXMF_TCP_TARGET ?? null) : null,
      nativeLifetime: "process",
    }),
};
