import type { RuntimeProvider } from "./runtime-provider.types";

export const runtimeProvider = {
  availability: {
    type: "unavailable",
    platform: "android",
    reason: "notImplemented",
  },
} satisfies RuntimeProvider;
