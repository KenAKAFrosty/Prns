import type { RuntimeProvider } from "./runtime-provider.types";

export const runtimeProvider: RuntimeProvider = {
  availability: {
    type: "unavailable",
    platform: "web",
    reason: "notImplemented",
  },
};
