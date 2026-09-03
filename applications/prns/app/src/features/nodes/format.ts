export function formatBytes(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function formatRequestKind(kind: "announceSelf" | "describe"): string {
  switch (kind) {
    case "announceSelf":
      return "Share node address";
    case "describe":
      return "View node information";
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
