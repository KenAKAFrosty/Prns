import { Link } from "expo-router";
import { useEffect, useState } from "react";
import { prns_host_uniffi as host, prns_service_runtime as native } from "@prns/app-bindings";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import * as platform from "@prns/app-platform";
import type { PlatformStatus } from "@prns/app-platform";
import { hex } from "../bytes";
import { useRouteActive } from "../navigation";
import { useInstallation } from "../runtime/InstallationProvider";
import { Button, ListRow, PageHeader, Panel, Screen, Text } from "../ui/primitives";

type Capture = {
  owner: owners.ServiceRuntimeLike | undefined;
  networkKey: string | undefined;
  ready: boolean;
  platform: PlatformStatus | undefined;
  inspection: host.HostInspection | undefined;
  primary: string | undefined;
  controller: string | undefined;
  incomplete: boolean;
};

function bytes(value: bigint): string {
  if (value < 1024n) return `${value} B`;
  if (value < 1048576n) return `${value / 1024n} KiB`;
  return `${value / 1048576n} MiB`;
}
function observed<T>(result: PromiseSettledResult<T>, previous: T | undefined): T | undefined {
  return result.status === "fulfilled" ? result.value : previous;
}

/** Read-only public facts, scoped to one local owner and host generation. */
export function ThisPhoneScreen() {
  const { owner, status, network, networkKey } = useInstallation();
  const active = useRouteActive();
  const ready = network?.phase === native.ServiceRuntimeNetworkPhase.Ready;
  const [capture, setCapture] = useState<Capture>();
  const [advanced, setAdvanced] = useState(false);
  const current = capture?.owner === owner && capture?.networkKey === networkKey && capture?.ready === ready ? capture : undefined;
  useEffect(() => {
    if (!active) return;
    const abort = new AbortController();
    let reading = false;
    const read = async () => {
      if (reading) return;
      reading = true;
      // Borrow only the current network generation. Offline local data stays open.
      const borrowed = (async () => ready && owner !== undefined ? owner.host() : undefined)();
      const [os, inspection, primary, controller] = await Promise.allSettled([
        platform.status(),
        (async () => (await borrowed)?.inspection({ signal: abort.signal }))(),
        (async () => { const client = await borrowed; return client === undefined ? undefined : hex(client.identityHash()); })(),
        (async () => {
          const client = await borrowed;
          if (client === undefined) return undefined;
          const result = await client.remoteControlControllerIdentity({ signal: abort.signal });
          if (result.tag !== host.RemoteControlControllerIdentitySettlement_Tags.Completed) throw new Error("Controller identity unavailable");
          return hex(host.remoteControlControllerIdentityHash(result.inner.value).value);
        })(),
      ]);
      if (!abort.signal.aborted) setCapture((previous) => {
        const same = previous?.owner === owner && previous?.networkKey === networkKey && previous?.ready === ready ? previous : undefined;
        return { owner, networkKey, ready, platform: observed(os, same?.platform),
          inspection: observed(inspection, same?.inspection), primary: observed(primary, same?.primary), controller: observed(controller, same?.controller),
          incomplete: [os, inspection, primary, controller].some((result) => result.status === "rejected") };
      });
      reading = false;
    };
    void read();
    const timer = setInterval(() => { void read(); }, 2000);
    return () => { abort.abort(); clearInterval(timer); };
  }, [active, owner, networkKey, ready]);
  const snapshot = current?.inspection?.host;
  const bluetooth = current?.platform?.bluetooth;
  return <Screen>
    <PageHeader title="This phone" subtitle="Local status and connections" help={{ label: "About this phone", content: <Text>This phone runs the local messaging and node controller. Public identity fingerprints identify it on the network; they are not secret keys. Connection counts do not prove message delivery.</Text> }} />
    <Panel>
      <Text>Saved data: {status === undefined ? "Not opened" : native.ServiceRuntimePhase[status.phase]}</Text>
      <Text>Networking: {network === undefined ? "Not started" : native.ServiceRuntimeNetworkPhase[network.phase]}</Text>
      {snapshot !== undefined && <Text>Network host: {snapshot.runtime.running ? "Running" : "Stopped"}</Text>}
      {!ready && <Text>Live network details are unavailable. Saved history and contacts remain available while local data is ready.</Text>}
      {(status?.failure !== undefined || network?.failure !== undefined) && <Text accessibilityRole="alert">A local or network failure is reported. Phone setup shows the recovery options.</Text>}
    </Panel>
    <Panel>
      <Text accessibilityRole="header">Connections</Text>
      {current === undefined && <Text>Checking this phone…</Text>}
      {current?.incomplete && <Text accessibilityRole="alert">Some details could not update. Previously observed values remain visible while this page retries automatically.</Text>}
      {bluetooth !== undefined && <>
        <Text>Bluetooth: {bluetooth.attached ? "Attached" : bluetooth.selected ? "Selected, not attached" : "Not selected"}</Text>
        <Text>Radio: {bluetooth.availability} · Permission: {bluetooth.authorization}</Text>
      </>}
      {snapshot !== undefined && <>
        <Text>{snapshot.runtime.onlineInterfaceCount} connections online · {snapshot.activeLinkCount} active links</Text>
        {snapshot.interfaces.length === 0 && <Text>No network connections configured.</Text>}
        {snapshot.interfaces.map((connection) => <Text key={hex(connection.interfaceId)}>{connection.name ?? (connection.kind === undefined ? "Connection" : host.InterfaceKind[connection.kind])} · {host.InterfaceHealth[connection.health]}</Text>)}
      </>}
    </Panel>
    <Link href="/more/interfaces" asChild><ListRow title="Connections" subtitle="Manage Bluetooth, local network and TCP" /></Link>
    <Link href="/more/network" asChild><ListRow title="Network activity" subtitle="Inspect connections, routes and announcements" /></Link>
    <Button title={advanced ? "Hide advanced details" : "Show advanced details"} tone="quiet" accessibilityState={{ expanded: advanced }} onPress={() => setAdvanced((value) => !value)} />
    {advanced && <Panel>
      <Text accessibilityRole="header">Public identities</Text>
      <Text>Primary network identity</Text><Text selectable>{current?.primary ?? "Unavailable"}</Text>
      <Text>Node controller identity</Text><Text selectable>{current?.controller ?? "Unavailable"}</Text>
      <Text>Messaging identities are managed separately. These fingerprints do not grant access to nodes.</Text>
      {snapshot !== undefined && <>
        <Text accessibilityRole="header">Network totals</Text>
        <Text>Received {bytes(snapshot.runtime.rxBytes)} · sent {bytes(snapshot.runtime.txBytes)}</Text>
        <Text>{snapshot.routes.length} known routes · {snapshot.activeLinkCount} active links</Text>
        <Text>Supported connections: {snapshot.backend.interfaceKinds.map((kind) => host.InterfaceKind[kind]).join(", ") || "None reported"}</Text>
        <Text>Host capabilities: {snapshot.backend.capabilities.map((capability) => host.Capability[capability]).join(", ") || "None reported"}</Text>
        <Text>Supported connections are capabilities, not proof that a connection is enabled or connected.</Text>
      </>}
    </Panel>}
  </Screen>;
}
