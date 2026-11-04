import { prns_host_uniffi as host } from "@prns/app-bindings";
import type { PlatformStatus } from "@prns/app-platform";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";

export type SupportFacts = Readonly<{
  bluetooth?: PlatformStatus["bluetooth"];
  notifications?: PlatformStatus["notifications"];
  onlineConnections?: number; physicalPeers?: number; routes?: number; links?: number;
}>;
/** One host capture supplies counts. Failed or offline reads remain unknown. */
export async function collectSupportFacts(status: () => Promise<PlatformStatus>, owner: owners.ServiceRuntimeLike | undefined,
  networkReady: boolean, signal: AbortSignal): Promise<SupportFacts> {
  const [platform, inspection] = await Promise.all([
    (async () => { try { return await status(); } catch { return undefined; } })(),
    (async () => { if (!networkReady || owner === undefined) return undefined;
      try { return await owner.host().inspection({ signal }); } catch { return undefined; }
    })(),
  ]);
  return {
    ...(platform === undefined ? {} : { bluetooth: platform.bluetooth, notifications: platform.notifications }),
    ...(inspection === undefined ? {} : { onlineConnections: inspection.host.runtime.onlineInterfaceCount,
      physicalPeers: inspection.interfaces.filter((entry) => entry.role === host.HostPhysicalRole.Member).length,
      routes: inspection.host.routes.length, links: inspection.host.activeLinkCount }),
  };
}
export type SupportBuild = Readonly<{
  version: string; nativeBuild: string; label: string; variant: string; platform: string; api: number;
  local: string; network: string; localFailure: boolean; networkFailure: boolean;
}>;
/** Never serialize native objects/errors: they can contain addresses or local paths. */
export function supportReport(build: SupportBuild, facts: SupportFacts): string {
  const yesNo = (value: boolean | undefined) => value === undefined ? "unknown" : value ? "yes" : "no";
  const count = (value: number | undefined) => value === undefined ? "unknown" : String(value);
  return ["PRNS Chat diagnostics", `App: ${build.version}`, `Native build: ${build.nativeBuild}`,
    `Source label: ${build.label}`, `Variant: ${build.variant}`, `Platform: ${build.platform}`, `App API: ${build.api}`,
    `Local data: ${build.local}`, `Networking: ${build.network}`,
    `Startup/cleanup failure reported: ${yesNo(build.localFailure)}`, `Network failure reported: ${yesNo(build.networkFailure)}`,
    `Bluetooth selected: ${yesNo(facts.bluetooth?.selected)}`, `Bluetooth attached: ${yesNo(facts.bluetooth?.attached)}`,
    `Bluetooth permission: ${facts.bluetooth?.authorization ?? "unknown"}`, `Bluetooth radio: ${facts.bluetooth?.availability ?? "unknown"}`,
    `Notification permission: ${facts.notifications ?? "unknown"}`, `Online connections: ${count(facts.onlineConnections)}`,
    `Physical peer connections: ${count(facts.physicalPeers)}`, `Known routes: ${count(facts.routes)}`, `Active links: ${count(facts.links)}`,
    "Counts describe one observation and do not prove message delivery.",
    "This report omits identities, contacts, messages, addresses, credentials, discovery-group names and local file paths."].join("\n");
}
