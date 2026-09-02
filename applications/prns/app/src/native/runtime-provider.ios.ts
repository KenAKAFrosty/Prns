import { developmentRuntime, scopedDevelopmentRuntime } from "@prns-internal/expo";
import type { RuntimeProvider } from "./runtime-provider.types";

export const runtimeProvider = {
  availability: { type: "available", platform: "ios" },
  runtime: developmentRuntime,
  acquire: (options) => scopedDevelopmentRuntime(developmentRuntime, options),
} satisfies RuntimeProvider;
