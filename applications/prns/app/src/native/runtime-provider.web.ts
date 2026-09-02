import type { RuntimeProvider } from "./runtime-provider.types";

export const runtimeProvider = {
  availability: {
    type: "unavailable",
    platform: "web",
    reason: "notImplemented",
  },
} satisfies RuntimeProvider;
