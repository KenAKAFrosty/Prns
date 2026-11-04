import { Button, Text } from "../ui/primitives";
import { useEffect, useMemo, useRef, useState } from "react";
import { View } from "react-native";
import { prns_chat_app as app, prns_host_uniffi as rc } from "@prns/app-bindings";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { Access } from "./Access";
import { Manage } from "./Manage";
import type { NodeDetailTab } from "./Manage";
import { Operations } from "./Operations";
import { Pairing } from "./Pairing";
import { detail, isTargetOperation, operationPending } from "./presentation";

export const nodeSections = ["Nodes", "Access", "Pairing", "Changes"] as const;
export type NodeSection = typeof nodeSections[number];

export function Nodes({ owner, active, networkKey, section, targetId, detailTab, onSectionChange, onOpenTarget, onDetailTabChange }: {
  owner: owners.ServiceRuntimeLike; active: boolean; networkKey?: string | undefined;
  section?: NodeSection | undefined; targetId?: string | undefined; detailTab?: NodeDetailTab | undefined;
  onSectionChange?: ((section: NodeSection) => void) | undefined;
  onOpenTarget?: ((id: string) => void) | undefined;
  onDetailTabChange?: ((tab: NodeDetailTab) => void) | undefined;
}) {
  const styles = useStyles();
  const borrowed = useMemo(() => {
    try { return { client: owner.host() }; }
    catch (cause) { return { error: detail(cause) }; }
  }, [owner, networkKey]);
  const client = borrowed.client;
  const retained = useMemo(() => {
    try { return { operations: app.appRemoteControlOperations() }; }
    catch (cause) { return { error: detail(cause) }; }
  }, [owner]);
  const [targets, setTargets] = useState<rc.RemoteControlIdentityHash[]>([]);
  const [records, setRecords] = useState<rc.RemoteControlOperationSnapshot[]>([]);
  const [selected, setSelected] = useState<rc.RemoteControlIdentityHash>();
  const [localTab, setLocalTab] = useState<NodeSection>("Nodes");
  const tab = section ?? localTab;
  const setTab = onSectionChange ?? setLocalTab;
  const [loaded, setLoaded] = useState(false);
  const routedTarget = useMemo(() => targetId === undefined ? undefined : {
    value: Uint8Array.from(targetId.match(/.{2}/g) ?? [], (byte) => parseInt(byte, 16)),
  }, [targetId]);
  const selectedTarget = routedTarget === undefined ? selected : targets.some((target) => hex(target.value) === targetId) ? routedTarget : undefined;
  const [forgetting, setForgetting] = useState<rc.RemoteControlIdentityHash>();
  const forgettingNow = useRef(false);
  const liveClient = useRef(client); liveClient.current = client;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [operationsError, setOperationsError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const changed = () => setRevision((value) => value + 1);
  useEffect(() => {
    setTargets([]); setLoaded(false); setSelected(undefined); setForgetting(undefined); setRecords([]); setError(undefined); setOperationsError(undefined);
    forgettingNow.current = false; setBusy(false);
  }, [client]);
  useEffect(() => {
    const operations = retained.operations;
    if (!active || client === undefined || operations === undefined || (tab !== "Nodes" && tab !== "Changes")) return;
    const read = () => {
      try { setRecords(operations.operations()); setOperationsError(undefined); }
      catch (cause) { setOperationsError(detail(cause)); }
    };
    read(); const timer = setInterval(read, 2000);
    return () => clearInterval(timer);
  }, [active, client, retained, tab, revision]);
  useEffect(() => {
    // Inventory and controller-card reads share native admission. Hidden inventory
    // must not compete with the selected Access or Pairing workflow.
    if (!active || client === undefined || tab !== "Nodes") return;
    const abort = new AbortController(); let reading = false;
    const read = async () => {
      if (reading) return;
      reading = true;
      try {
        const result = await client.remoteControlTargetInventory({ signal: abort.signal });
        if (abort.signal.aborted) return;
        if (result.tag === rc.RemoteControlTargetInventorySettlement_Tags.Failed) throw result.inner.failure;
        setTargets(result.inner.value.targets); setLoaded(true); setError(undefined);
      } catch (cause) { if (!abort.signal.aborted) setError(detail(cause)); }
      finally { reading = false; }
    };
    void read(); const timer = setInterval(() => { void read(); }, 2000);
    return () => { abort.abort(); clearInterval(timer); };
  }, [active, client, tab, revision]);
  async function forget() {
    if (!active || client === undefined || forgetting === undefined || busy || forgettingNow.current) return;
    forgettingNow.current = true;
    const target = forgetting; setBusy(true); setError(undefined);
    try {
      const result = await client.forgetRemoteControlTarget(target);
      if (liveClient.current !== client) return;
      if (result.tag === rc.RemoteControlForgetRemoteControlTargetSettlement_Tags.Failed) throw result.inner.failure;
      if (hex(selected?.value ?? new Uint8Array()) === hex(target.value)) setSelected(undefined);
      setForgetting(undefined); changed();
    } catch (cause) { if (liveClient.current === client) setError(`Removal was not confirmed: ${detail(cause)}. Check saved node status before trying again.`); }
    finally { if (liveClient.current === client) { forgettingNow.current = false; setBusy(false); } }
  }
  if (client === undefined) return <Text accessibilityRole="alert" style={styles.error}>Node management is unavailable: {borrowed.error}. Start networking from Interfaces before continuing.</Text>;
  return <View style={styles.section}>
    <Text>Pair with a node or import its access card to manage it. Node access is separate from your messaging contacts.</Text>
    <View style={styles.row} accessibilityRole="tablist">{nodeSections.map((value) => <Button key={value} title={value} accessibilityState={{ selected: tab === value }} onPress={() => setTab(value)} />)}</View>
    {(tab === "Nodes" || tab === "Changes") && (operationsError ?? retained.error) !== undefined &&
      <Text accessibilityRole="alert" style={styles.error}>{operationsError ?? retained.error}</Text>}
    <View style={tab === "Nodes" ? styles.section : styles.hidden}>
      {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
      {error !== undefined && <Button title={forgetting === undefined ? "Retry loading saved nodes" : "Check saved node status"} disabled={busy || !active} onPress={changed} />}
      {targetId !== undefined && selectedTarget === undefined && <Text accessibilityRole={loaded ? "alert" : undefined}>{loaded ? "This node is not saved on this installation. Return to Nodes to choose a saved node or import its access card." : "Loading saved node…"}</Text>}
      {targetId === undefined && <>
      {targets.length === 0 && <Text>No saved nodes. Import an access card or pair with a node.</Text>}
      {targets.map((target) => <View style={styles.card} key={hex(target.value)}>
        <Text selectable style={styles.fingerprint}>{hex(target.value)}</Text>
        <Button title={`Manage ${hex(target.value).slice(0, 12)}`} onPress={() => onOpenTarget === undefined ? setSelected(target) : onOpenTarget(hex(target.value))} />
        <Button title={`Review forgetting ${hex(target.value).slice(0, 12)}`} disabled={busy || records.some((record) => isTargetOperation(record, target.value) && operationPending(record.state))} onPress={() => setForgetting(target)} />
      </View>)}
      {forgetting !== undefined && <View style={styles.card}>
        <Text>Forget saved access to {hex(forgetting.value)} on this installation? This is durable local removal. It does not revoke this controller at the remote node, cancel a submitted change, or undo any remote setting. Ask the node administrator to revoke permission there if needed.</Text>
        <Button title="Keep saved node" disabled={busy} onPress={() => setForgetting(undefined)} />
        <Button title="Forget this saved node" disabled={busy || !active} onPress={() => { void forget(); }} />
      </View>}
      </>}
      {selectedTarget !== undefined && retained.operations !== undefined && <Manage key={hex(selectedTarget.value)} client={client} target={selectedTarget} operations={retained.operations} records={records} active={active && tab === "Nodes"} changed={changed} tab={detailTab} onTabChange={onDetailTabChange} />}
    </View>
    <View style={tab === "Access" ? styles.section : styles.hidden}><Access client={client} active={active && tab === "Access"} changed={changed} /></View>
    <View style={tab === "Pairing" ? styles.section : styles.hidden}><Pairing client={client} active={active && tab === "Pairing"} changed={changed} onOpenPaired={(target) => {
      if (!active || liveClient.current !== client) return;
      if (onOpenTarget !== undefined) onOpenTarget(hex(target.value));
      else { setSelected(target); setTab("Nodes"); }
    }} /></View>
    <View style={tab === "Changes" ? styles.section : styles.hidden}>{retained.operations !== undefined && <Operations operations={retained.operations} records={records} changed={changed} />}</View>
  </View>;
}
