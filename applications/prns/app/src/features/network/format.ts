import type { DevelopmentNodeSnapshot, LocalNetworkRouteSnapshot } from "@prns-internal/expo";

import { formatBytes } from "@/features/nodes/format";

/** Native durations describe the last inspection; never advance them with the JS clock. */
export function formatNetworkDuration(millis: bigint): string {
  if (millis < 1_000n) return "<1s";
  const seconds = millis / 1_000n;
  if (seconds < 60n) return `${seconds}s`;
  const minutes = seconds / 60n;
  if (minutes < 60n) return `${minutes}m`;
  const hours = minutes / 60n;
  if (hours < 24n) return `${hours}h ${minutes % 60n}m`;
  return `${hours / 24n}d ${hours % 24n}h`;
}

export function formatRouteExpiry(route: LocalNetworkRouteSnapshot): string {
  return route.expired ? "Expired" : `Expires in ${formatNetworkDuration(route.expiresInMillis)}`;
}

export function shortNetworkId(bytes: Uint8Array): string {
  const value = formatBytes(bytes);
  return value.length <= 12 ? value : `${value.slice(0, 8)}…${value.slice(-4)}`;
}

export function formatTraffic(bytes: bigint): string {
  if (bytes < 1_024n) return `${bytes} B`;
  if (bytes < 1_048_576n) return `${bytes / 1_024n} KiB`;
  return `${bytes / 1_048_576n} MiB`;
}

export function hopLabel(hops: number): string {
  return `${hops} ${hops === 1 ? "hop" : "hops"}`;
}

export function logicalInterfaceName(snapshot: DevelopmentNodeSnapshot, id: Uint8Array): string {
  const localHost = snapshot.localHost;
  if (localHost.tag !== "Running") return "Interface details unavailable";
  const networkInterface = localHost.inner.host.interfaces.find(
    (candidate) => formatBytes(candidate.interfaceId) === formatBytes(id),
  );
  if (networkInterface?.kind === "AutomaticBluetoothLe") return "Bluetooth";
  return networkInterface?.name ?? networkInterface?.kind ?? "Interface not listed";
}

/** Resolve only the recorded ingress, never a destination's present-day route. */
export function announceIngressName(snapshot: DevelopmentNodeSnapshot, id: Uint8Array): string {
  const peer = snapshot.bluetooth.peers.find(
    (candidate) => formatBytes(candidate.interfaceId) === formatBytes(id),
  );
  if (peer !== undefined) return peer.name ?? "Bluetooth connection";
  const logicalName = logicalInterfaceName(snapshot, id);
  return logicalName === "Interface not listed" ? "Interface no longer listed" : logicalName;
}
