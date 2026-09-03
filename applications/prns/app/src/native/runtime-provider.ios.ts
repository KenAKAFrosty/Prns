import { developmentRuntime, scopedDevelopmentRuntime } from "@prns-internal/expo";
import type { RuntimeProvider } from "./runtime-provider.types";

export const runtimeProvider: RuntimeProvider = {
  availability: { type: "available", platform: "ios" },
  runtime: developmentRuntime,
  acquire: (options) => scopedDevelopmentRuntime(developmentRuntime, options),
};
