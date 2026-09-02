export function formatBytes(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function formatRequestKind(kind: "announceSelf" | "describe"): string {
  switch (kind) {
    case "announceSelf":
      return "Announce self";
    case "describe":
      return "Describe";
  }
}

export function formatRuntime(runtime: "failed" | "running" | "starting" | "stopped" | "stopping") {
  switch (runtime) {
    case "failed":
      return "Failed";
    case "running":
      return "Running";
    case "starting":
      return "Starting";
    case "stopped":
      return "Stopped";
    case "stopping":
      return "Stopping";
  }
}

export function formatBluetooth(
  bluetooth:
    | { readonly type: "degraded" | "unavailable"; readonly detail: string }
    | { readonly type: "disabled" | "notCompiled" | "preparing" | "ready" },
): string {
  switch (bluetooth.type) {
    case "degraded":
      return `Degraded — ${bluetooth.detail}`;
    case "disabled":
      return "Disabled";
    case "notCompiled":
      return "Not compiled for this platform";
    case "preparing":
      return "Preparing";
    case "ready":
      return "Ready";
    case "unavailable":
      return `Unavailable — ${bluetooth.detail}`;
  }
}
