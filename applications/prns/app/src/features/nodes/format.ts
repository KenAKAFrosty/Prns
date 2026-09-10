import { DevelopmentNodeRuntime, RemoteControlRequestKind } from "@prns-internal/expo";
export function formatBytes(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function formatRequestKind(kind: RemoteControlRequestKind): string {
  switch (kind) {
    case RemoteControlRequestKind.AnnounceSelf:
      return "Share node address";
    case RemoteControlRequestKind.Describe:
      return "View node information";
  }
}

export function formatRuntime(runtime: DevelopmentNodeRuntime) {
  switch (runtime) {
    case DevelopmentNodeRuntime.Failed:
      return "Failed";
    case DevelopmentNodeRuntime.Running:
      return "Running";
    case DevelopmentNodeRuntime.Starting:
      return "Starting";
    case DevelopmentNodeRuntime.Stopped:
      return "Stopped";
    case DevelopmentNodeRuntime.Stopping:
      return "Stopping";
  }
}
