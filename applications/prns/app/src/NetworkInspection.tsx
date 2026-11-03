import { Disclosure } from "./ui/Disclosure";
import { Actions, ActionMenu, Button, PageHeader, Text } from "./ui/primitives";
import { useEffect, useMemo, useState } from "react";
import { ActivityIndicator, View } from "react-native";
import { prns_host_uniffi as host } from "@prns/app-bindings";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { hex } from "./bytes";
import { useStyles } from "./ui";
import { Facts } from "./nodes/presentation";
import type { NetworkActivity } from "./runtime/network-activity";

function bytes(value: bigint): string {
  if (value < 1024n) return `${value} B`;
  if (value < 1048576n) return `${value / 1024n} KiB`;
  return `${value / 1048576n} MiB`;
}

/** Snapshot inspection never claims a route proves delivery of a message. */
export function NetworkInspection({ owner, active, networkKey, activity, clearActivity }: {
  owner: owners.ServiceRuntimeLike; active: boolean; networkKey?: string | undefined;
  activity?: NetworkActivity | undefined; clearActivity?: (() => void) | undefined;
}) {
  const styles = useStyles();
  const borrowed = useMemo(() => {
    try { return { client: owner.host() }; }
    catch (cause) { return { error: cause instanceof Error ? cause.message : String(cause) }; }
  }, [owner, networkKey]);
  const client = borrowed.client;
  const [inspection, setInspection] = useState<host.HostInspection>();
  const snapshot = inspection?.host;
  const [error, setError] = useState<string>();
  const [tab, setTab] = useState<"Connections" | "Physical peers" | "Routes" | "Known identities" | "Announcements">("Connections");
  const [visible, setVisible] = useState(50);
  const [refresh, setRefresh] = useState(0);
  useEffect(() => { setInspection(undefined); setError(undefined); }, [client]);
  useEffect(() => {
    if (!active || client === undefined) return;
    const abort = new AbortController();
    let reading = false;
    const read = async () => {
      if (reading) return;
      reading = true;
      try {
        const current = await client.inspection({ signal: abort.signal });
        if (!abort.signal.aborted) { setInspection(current); setError(undefined); }
      } catch (cause) { if (!abort.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause)); }
      finally { reading = false; }
    };
    void read();
    const timer = setInterval(() => { void read(); }, 2000);
    return () => { abort.abort(); clearInterval(timer); };
  }, [active, client, refresh]);
  return <View style={styles.section}>
    <PageHeader title="Network activity" trailing={<ActionMenu label="Network details" items={[
        { title: "Physical peers", onPress: () => { setTab("Physical peers"); setVisible(50); } },
        { title: "Known identities", onPress: () => { setTab("Known identities"); setVisible(50); } },
        ...(tab === "Announcements" ? [{ title: "Clear displayed announcements", disabled: clearActivity === undefined, onPress: () => clearActivity?.() }] : []),
      ]} />} />
    <Text>Connections and routes describe the current network. Message delivery is shown separately in each conversation.</Text>
    {borrowed.error !== undefined && <Text>Networking is stopped or unavailable. Saved data remains available.</Text>}
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
    {client !== undefined && snapshot === undefined && error === undefined && <ActivityIndicator accessibilityLabel="Loading network status" />}
    {error !== undefined && <Button title="Retry network inspection" onPress={() => { setRefresh((value) => value + 1); }} />}
    {snapshot !== undefined && <>
      <Text>{snapshot.runtime.onlineInterfaceCount} connections online · {snapshot.activeLinkCount} active links</Text>
      <Disclosure label="Traffic totals">
        <Text>Received {bytes(snapshot.runtime.rxBytes)} · sent {bytes(snapshot.runtime.txBytes)}</Text>
        <Text>{bytes(snapshot.runtime.rxBps)}/s in · {bytes(snapshot.runtime.txBps)}/s out</Text>
      </Disclosure>
      {snapshot.persistence.lastFailureDetail !== undefined && <Text accessibilityRole="alert" style={styles.error}>{snapshot.persistence.lastFailureDetail}</Text>}
      <Actions>
        {(["Connections", "Routes", "Announcements"] as const).map((name) => {
          const count = name === "Connections" ? snapshot.interfaces.length : name === "Routes" ? snapshot.routes.length : activity?.announcements.length ?? 0;
          return <Button key={name} title={`${name} (${count})`} accessibilityLabel={name} tone="quiet" size="compact"
            accessibilityState={{ selected: tab === name }} onPress={() => { setTab(name); setVisible(50); }} />;
        })}
      </Actions>
      {(tab === "Physical peers" || tab === "Known identities") && <Text accessibilityRole="header" style={styles.title}>{tab}</Text>}
      {tab === "Connections" && <>
        {snapshot.interfaces.length === 0 && <Text>No connections configured.</Text>}
        {snapshot.interfaces.map((connection) => <View key={hex(connection.interfaceId)} style={styles.card}>
          <Text style={styles.title}>{connection.name ?? "Unnamed connection"}</Text>
          <Text>{connection.kind === undefined ? "Connection" : host.InterfaceKind[connection.kind]} · {host.InterfaceHealth[connection.health]}</Text>
          {connection.failureDetail !== undefined && <Text accessibilityRole="alert">{connection.failureDetail}</Text>}
          <Disclosure label="Details" accessibilityLabel={`Connection details: ${connection.name ?? "Unnamed connection"}`}>
            <Text>Received {bytes(connection.rxBytes)} · sent {bytes(connection.txBytes)}</Text>
            <Text>{connection.routeCount} routes · {connection.linkCount} links</Text>
          </Disclosure>
        </View>)}
      </>}
      {tab === "Physical peers" && <>
        <Text>Nearby devices and the connections used to reach them. Message delivery is shown in each conversation.</Text>
        {inspection!.interfaces.length === 0 && <Text>No physical interfaces currently reported.</Text>}
        {inspection!.interfaces.slice(0, visible).map((peer) => <View key={`${hex(peer.runtimeInterface)}:${peer.attachmentEpoch}`} style={styles.card}>
          <Text style={styles.title}>{peer.name ?? peer.endpoint ?? "Network connection"}</Text>
          <Text>{host.HostPhysicalRole[peer.role]} · {host.HostPhysicalOrigin[peer.origin]} · {host.InterfaceHealth[peer.health]}</Text>
          <Disclosure label="Details" accessibilityLabel={`Physical peer details: ${peer.name ?? peer.endpoint ?? "Network connection"}`}>
          <Text selectable style={styles.fingerprint}>Interface: {hex(peer.runtimeInterface)}</Text>
          {peer.parent !== undefined && <Text selectable>Parent: {hex(peer.parent)}</Text>}
          {peer.endpoint !== undefined && <Text selectable>{peer.endpoint}</Text>}
          {peer.failureDetail !== undefined && <Text accessibilityRole="alert">{peer.failureDetail}</Text>}
          <Text>Received {bytes(peer.rxBytes)} · sent {bytes(peer.txBytes)}</Text>
          <Facts value={peer.radio} /><Facts value={peer.details} />
          </Disclosure>
        </View>)}
        {inspection!.interfaces.length > visible && <Button title="Show more physical peers" onPress={() => setVisible((value) => value + 50)} />}
      </>}
      {tab === "Routes" && <>
        {snapshot.routes.length === 0 && <Text>No routes currently known.</Text>}
        {snapshot.routes.slice(0, visible).map((route) => <View key={`${hex(route.destination)}:${hex(route.interfaceId)}`} style={styles.card}>
          <Text style={styles.title}>Destination {hex(route.destination).slice(0, 8)}</Text>
          <Text>{route.hops} hops via {snapshot.interfaces.find((connection) => hex(connection.interfaceId) === hex(route.interfaceId))?.name ?? hex(route.interfaceId)}</Text>
          <Disclosure label="Route details" accessibilityLabel={`Route details: ${hex(route.destination).slice(0, 8)}`}>
          <Text selectable style={styles.fingerprint}>{hex(route.destination)}</Text>
          {route.viaIdentity !== undefined && <Text selectable style={styles.fingerprint}>Next identity: {hex(route.viaIdentity)}</Text>}
          <Text>Learned {elapsed(inspection!.engineTimeMillis - route.learnedAtMillis)} ago · last activity {route.lastRouteActivityAtMillis === 0n ? "unknown" : `${elapsed(inspection!.engineTimeMillis - route.lastRouteActivityAtMillis)} ago`}</Text>
          <Text>{route.expiresAtMillis <= inspection!.engineTimeMillis ? "Expiry reached; awaiting route cleanup" : `Expires in ${elapsed(route.expiresAtMillis - inspection!.engineTimeMillis)}`}</Text>
          </Disclosure>
        </View>)}
        {snapshot.routes.length > visible && <Button title="Show more routes" onPress={() => { setVisible((value) => value + 50); }} />}
      </>}
      {tab === "Announcements" && <>
        <Text>Recent announcements heard on this connection. Older entries are removed, and the list is not saved.</Text>
        {activity?.error !== undefined && <Text accessibilityRole="alert">{activity.error}</Text>}
        {activity !== undefined && <Text>{activity.announcements.length} shown · {activity.evicted} older entries removed · {activity.dropped.toString()} events not captured</Text>}
        {(activity?.announcements.length ?? 0) === 0 && <Text>No announcements observed in this window.</Text>}
        {activity?.announcements.slice(0, visible).map((item) => {
          const contact = activity.discovered?.find((value) => hex(value.observation.event.inner.destination) === hex(item.event.inner.destination));
          const connection = snapshot.interfaces.find((value) => hex(value.interfaceId) === hex(item.event.inner.sourceInterface))?.name ?? "Unknown connection";
          return <View key={item.id.toString()} style={styles.card}>
            <Text style={styles.title}>{contact?.contact.claims.displayName || "Network announcement"}</Text>
            <Text>{new Date(item.observedAt).toLocaleTimeString()} · {item.event.inner.hops} hops · {connection}</Text>
            <Disclosure label="Details" accessibilityLabel={`Announcement details: ${item.id}`}>
              <Text selectable style={styles.fingerprint}>Destination: {hex(item.event.inner.destination)}</Text>
              <Text selectable style={styles.fingerprint}>Source interface: {hex(item.event.inner.sourceInterface)}</Text>
              <Text selectable>Public app data: {hex(item.event.inner.appData)}</Text>
            </Disclosure>
          </View>;
        })}
        {(activity?.announcements.length ?? 0) > visible && <Button title="Show more announcements" onPress={() => setVisible((value) => value + 50)} />}
      </>}
      {tab === "Known identities" && <>
        <Text>Verified identities seen on the network. They are not added to your contacts automatically.</Text>
        {snapshot.destinationIdentities.length === 0 && <Text>No destination identities currently known.</Text>}
        {snapshot.destinationIdentities.slice(0, visible).map((destination) => <View key={hex(destination.destination)} style={styles.card}>
          <Text selectable style={styles.fingerprint}>Destination: {hex(destination.destination)}</Text>
          <Text selectable style={styles.fingerprint}>Identity: {hex(destination.identity)}</Text>
        </View>)}
        {snapshot.destinationIdentities.length > visible && <Button title="Show more identities" onPress={() => { setVisible((value) => value + 50); }} />}
      </>}
    </>}
  </View>;
}

function elapsed(millis: bigint): string {
  const seconds = (millis < 0n ? 0n : millis) / 1000n;
  return seconds < 60n ? `${seconds}s` : seconds < 3600n ? `${seconds / 60n}m` : `${seconds / 3600n}h`;
}
