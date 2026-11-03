import { Button, Text, TextInput } from "../ui/primitives";
import { useEffect, useRef, useState } from "react";
import { View } from "react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { RequestReview } from "./RequestReview";
import { Controllers } from "./Controllers";
import { Interface } from "./Interface";
import { Wifi } from "./Wifi";
import { detail, Facts, isTargetOperation, label, nodeRouteMissing, operationUncertain, requestLabel } from "./presentation";

const kind = (request: rc.RemoteControlRequest) => rc.RemoteControlRequestKind[request.tag as keyof typeof rc.RemoteControlRequestKind];

export const nodeDetailTabs = ["Overview", "Interfaces", "Controls", "Access"] as const;
export type NodeDetailTab = typeof nodeDetailTabs[number];

export function Manage({ client, target, operations, records, active, changed, tab: routedTab, onTabChange }: {
  client: rc.HostClientHandleLike; target: rc.RemoteControlIdentityHash; operations: rc.RemoteControlOperationsLike;
  records: rc.RemoteControlOperationSnapshot[]; active: boolean; changed(): void;
  tab?: NodeDetailTab | undefined; onTabChange?: ((tab: NodeDetailTab) => void) | undefined;
}) {
  const styles = useStyles();
  const [description, setDescription] = useState<rc.RemoteControlDescription>();
  const [response, setResponse] = useState<rc.RemoteControlResponse>();
  const [interfaces, setInterfaces] = useState<rc.RemoteControlInterfaceInventory>();
  const [configurations, setConfigurations] = useState<Map<string, rc.RemoteControlInterfaceConfigOutcome>>(new Map());
  const [discoveryGroups, setDiscoveryGroups] = useState<Map<string, rc.RemoteControlDiscoveryGroupsInventoryOutcome>>(new Map());
  const [radios, setRadios] = useState<Map<string, rc.RemoteControlRadioStatus>>(new Map());
  const [peerPages, setPeerPages] = useState<Map<string, rc.RemoteControlInterfacePeersOutcome>>(new Map());
  const [controllers, setControllers] = useState<rc.RemoteControlControllerInventory>();
  const [wifi, setWifi] = useState<rc.RemoteControlWifiTransactionStatus>();
  const [pending, setPending] = useState<rc.RemoteControlRequest>();
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [reading, setReading] = useState(false);
  const [missingRoute, setMissingRoute] = useState(false);
  const [name, setName] = useState("");
  const [localTab, setLocalTab] = useState<NodeDetailTab>("Overview");
  const tab = routedTab ?? localTab;
  const setTab = onTabChange ?? setLocalTab;
  const liveClient = useRef(client); liveClient.current = client;
  const liveReadScope = useRef({ target, active }); liveReadScope.current = { target, active };
  const describedClient = useRef<rc.HostClientHandleLike | undefined>(undefined);
  const submission = useRef(false);
  const readAbort = useRef<AbortController | undefined>(undefined);
  useEffect(() => {
    readAbort.current?.abort(); readAbort.current = undefined; setReading(false); setMissingRoute(false); setDescription(undefined); setPending(undefined); setResponse(undefined);
    setInterfaces(undefined); setConfigurations(new Map()); setDiscoveryGroups(new Map()); setRadios(new Map()); setPeerPages(new Map()); setControllers(undefined); setWifi(undefined); setError(undefined); setNotice(undefined);
    submission.current = false; describedClient.current = undefined;
    return () => { readAbort.current?.abort(); readAbort.current = undefined; };
  }, [client, target]);
  useEffect(() => { if (!active) { readAbort.current?.abort(); readAbort.current = undefined; setReading(false); setMissingRoute(false); setDescription(undefined); setPending(undefined); setWifi(undefined); } }, [active]);
  const uncertain = records.some((operation) => isTargetOperation(operation, target.value) && operationUncertain(operation.state));
  const can = (value: rc.RemoteControlRequestKind) => describedClient.current === client && (description?.availableRequests.kinds.includes(value) ?? false);
  async function read(request: rc.RemoteControlRequest, find = false) {
    const inScope = () => liveClient.current === client && liveReadScope.current.target === target && liveReadScope.current.active;
    if (!inScope() || reading || readAbort.current !== undefined) return;
    if (find && (!missingRoute || request.tag !== rc.RemoteControlRequest_Tags.Describe)) return;
    if (request.tag !== rc.RemoteControlRequest_Tags.Describe && !can(kind(request))) return;
    const abort = new AbortController(); readAbort.current = abort;
    const current = () => !abort.signal.aborted && inScope();
    setReading(true); setError(undefined); setMissingRoute(find);
    if (request.tag === rc.RemoteControlRequest_Tags.Describe) { setDescription(undefined); setPending(undefined); }
    let discovering = find;
    try {
      if (find) {
        // RequestPath refreshes even a known route, so only this explicit recovery
        // action discovers. Use the saved grant's canonical service destination.
        const resolved = await client.resolveRemoteControlTarget(target, { signal: abort.signal });
        if (!current()) return;
        if (resolved.tag === rc.RemoteControlResolveRemoteControlTargetSettlement_Tags.Failed) throw resolved.inner.failure;
        const found = await client.execute(new rc.HostCommand.RequestPath({ destination: resolved.inner.value.endpoint.destinationHash.value }), { signal: abort.signal });
        if (!current()) return;
        if (found.tag === rc.CommandSettlement_Tags.Failed) throw found.inner.failure;
        if (found.inner.outcome.tag !== rc.CommandOutcome_Tags.PathDiscovered) throw new Error("Discovery did not confirm a path to the node");
        discovering = false; setMissingRoute(false);
      }
      const result = await client.remoteControlTargetExchange(target.value, request, { signal: abort.signal });
      if (!current()) return;
      if (result.tag === rc.RemoteControlExchangeSettlement_Tags.Failed) {
        if (nodeRouteMissing(result.inner.failure)) {
          setMissingRoute(true);
          throw new Error("This node has not been discovered. Check its connection, then choose Find node.");
        }
        throw result.inner.failure;
      }
      if (request.tag === rc.RemoteControlRequest_Tags.InventoryInterfacePeers && result.inner.response.tag === rc.RemoteControlResponse_Tags.InventoryInterfacePeers && result.inner.response.inner.value.tag === rc.RemoteControlInterfacePeersOutcome_Tags.Page && hex(result.inner.response.inner.value.inner.value.id.value) !== hex(request.inner.id.value)) throw new Error("The node returned peers for a different interface. Read the node’s current interface list before using this result.");
      setResponse(result.inner.response);
      const value = result.inner.response;
      if (request.tag === rc.RemoteControlRequest_Tags.InventoryInterfaceConfig && value.tag === rc.RemoteControlResponse_Tags.InventoryInterfaceConfig) setConfigurations((current) => new Map(current).set(hex(request.inner.id.value), value.inner.value));
      if (request.tag === rc.RemoteControlRequest_Tags.InventoryInterfaceDiscoveryGroups && value.tag === rc.RemoteControlResponse_Tags.InventoryInterfaceDiscoveryGroups) setDiscoveryGroups((current) => new Map(current).set(hex(request.inner.id.value), value.inner.value));
      if (request.tag === rc.RemoteControlRequest_Tags.InspectRadio && value.tag === rc.RemoteControlResponse_Tags.InspectRadio) setRadios((current) => new Map(current).set(hex(request.inner.id.value), value.inner.value));
      if (request.tag === rc.RemoteControlRequest_Tags.InventoryInterfacePeers && value.tag === rc.RemoteControlResponse_Tags.InventoryInterfacePeers) setPeerPages((current) => new Map(current).set(hex(request.inner.id.value), value.inner.value));
      if (request.tag === rc.RemoteControlRequest_Tags.InventoryControllers && value.tag === rc.RemoteControlResponse_Tags.InventoryControllers) setControllers(value.inner.value);
      if (request.tag === rc.RemoteControlRequest_Tags.Describe && value.tag === rc.RemoteControlResponse_Tags.Describe) { submission.current = false; describedClient.current = client; setDescription(value.inner.value); }
      if (request.tag === rc.RemoteControlRequest_Tags.InspectWifiTransaction && result.inner.response.tag === rc.RemoteControlResponse_Tags.InspectWifiTransaction) setWifi(result.inner.response.inner.value);
      if (request.tag === rc.RemoteControlRequest_Tags.InventoryInterfaces && result.inner.response.tag === rc.RemoteControlResponse_Tags.InventoryInterfaces) {
        const page = result.inner.response.inner.value;
        const ids = new Set(page.entries.map((entry) => hex(entry.id.value)));
        setInterfaces(page);
        setConfigurations((current) => new Map([...current].filter(([id]) => ids.has(id))));
        setDiscoveryGroups((current) => new Map([...current].filter(([id]) => ids.has(id))));
        setRadios((current) => new Map([...current].filter(([id]) => ids.has(id))));
        setPeerPages((current) => new Map([...current].filter(([id]) => ids.has(id))));
      }
    } catch (cause) {
      if (current()) setError(discovering ? `The node could not be found: ${detail(cause)}. Check its connection and try again.` : detail(cause));
    } finally {
      if (readAbort.current === abort) readAbort.current = undefined;
      if (current()) setReading(false);
    }
  }
  function review(request: rc.RemoteControlRequest) {
    if (!active || uncertain || !can(kind(request))) return;
    setPending(request); setError(undefined); setNotice(undefined);
  }
  function submit() {
    if (submission.current || pending === undefined || uncertain || !active || !can(kind(pending))) return;
    submission.current = true;
    try {
      const id = operations.submitTarget(client, target.value, pending);
      setNotice(`Change ${id} has been submitted. Check Changes for its result, then read the node’s current settings before making another change.`);
      setPending(undefined); setDescription(undefined); setWifi(undefined);
      setConfigurations(new Map()); setDiscoveryGroups(new Map()); setRadios(new Map()); setPeerPages(new Map()); setControllers(undefined); changed();
    } catch (cause) { setError(`No change ID was returned: ${detail(cause)}. Nothing is retried automatically.`); }
  }
  function readButton(title: string, request: rc.RemoteControlRequest) {
    return can(kind(request)) && <Button key={title} title={title} disabled={reading || !active} onPress={() => { void read(request); }} />;
  }
  function changeButton(title: string, request: rc.RemoteControlRequest) {
    return can(kind(request)) && <Button key={title} title={title} disabled={uncertain || !active || reading} onPress={() => review(request)} />;
  }
  function choices<T extends number>(values: Record<string, string | number>, build: (value: T) => rc.RemoteControlRequest, title: string) {
    return <View style={styles.row}>{Object.entries(values).filter(([, value]) => typeof value === "number").map(([key, value]) => changeButton(`${title}: ${label(key)}`, build(value as T)))}</View>;
  }
  return <View style={styles.section}>
    <Text style={styles.heading}>Manage node</Text><Text selectable style={styles.fingerprint}>{hex(target.value)}</Text>
    <Button title="Read node capabilities" disabled={reading || !active} onPress={() => { void read(new rc.RemoteControlRequest.Describe()); }} />
    {description === undefined ? <Text>Read node capabilities to see the available settings.</Text> : <Text>Available actions: {description.availableRequests.kinds.map(requestLabel).join(", ")}</Text>}
    {uncertain && <Text accessibilityRole="alert">A change is still pending or its result is unknown. Check Changes and the node’s current settings before making another change.</Text>}
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
    {missingRoute && <Button title={reading ? "Finding node…" : "Find node"} disabled={reading || !active} onPress={() => { void read(new rc.RemoteControlRequest.Describe(), true); }} />}
    {notice !== undefined && <Text accessibilityRole="alert">{notice}</Text>}
    <View style={styles.row} accessibilityRole="tablist">{nodeDetailTabs.map((value) => <Button key={value} title={value} accessibilityState={{ selected: tab === value }} onPress={() => setTab(value)} />)}</View>
    <View style={tab === "Overview" ? styles.section : styles.hidden}>
      {readButton("Read build information", new rc.RemoteControlRequest.DescribeBuild())}
      {readButton("Read power status", new rc.RemoteControlRequest.DescribePower())}
      {readButton("Read node name", new rc.RemoteControlRequest.DescribeNodeName())}
      {readButton("Read controller access", new rc.RemoteControlRequest.InventoryControllers({ page: new rc.RemoteControlControllerPage.First() }))}
      {controllers?.continuation.tag === rc.RemoteControlControllerContinuation_Tags.More && readButton("Read next controller page", new rc.RemoteControlRequest.InventoryControllers({ page: new rc.RemoteControlControllerPage.After({ value: controllers.continuation.inner.value }) }))}
      {readButton("Read Wi-Fi transaction", new rc.RemoteControlRequest.InspectWifiTransaction())}
    </View>
    <View style={tab === "Interfaces" ? styles.section : styles.hidden}>
      {readButton("Read interfaces", new rc.RemoteControlRequest.InventoryInterfaces({ page: new rc.RemoteControlInterfacePage.First() }))}
      {interfaces !== undefined && <>
        <Text>More interfaces may be available on the next page. The node’s settings can change while you browse.</Text>
        {interfaces.entries.map((entry) => <Interface key={hex(entry.id.value)} entry={entry} supported={description?.availableRequests ?? { kinds: [] }} configuration={configurations.get(hex(entry.id.value))} discovery={discoveryGroups.get(hex(entry.id.value))} radio={radios.get(hex(entry.id.value))} peers={peerPages.get(hex(entry.id.value))} disabled={reading || !active} blocked={uncertain} read={(request) => { void read(request); }} review={review} />)}
        {interfaces.continuation.tag === rc.RemoteControlInterfaceContinuation_Tags.More && readButton("Read next interface page", new rc.RemoteControlRequest.InventoryInterfaces({ page: new rc.RemoteControlInterfacePage.After({ value: interfaces.continuation.inner.value }) }))}
      </>}
    </View>
    <View style={tab === "Controls" ? styles.section : styles.hidden}>
      <Text>Changing power, radio mode, or interface settings may disconnect this session. Wake may require local access to the node.</Text>
      {can(rc.RemoteControlRequestKind.SetNodeName) && <><TextInput accessibilityLabel="New node name" value={name} onChangeText={setName} style={styles.input} />{changeButton("Review node name", new rc.RemoteControlRequest.SetNodeName({ name: { value: name } }))}</>}
      {choices<rc.RemoteControlSystemPower>(rc.RemoteControlSystemPower, (power) => new rc.RemoteControlRequest.SetSystemPower({ power }), "Node power")}
      {choices<rc.RemoteControlGnssPower>(rc.RemoteControlGnssPower, (power) => new rc.RemoteControlRequest.SetGnssPower({ power }), "GPS")}
      {choices<rc.RemoteControlDisplayVisibility>(rc.RemoteControlDisplayVisibility, (visibility) => new rc.RemoteControlRequest.SetDisplayVisibility({ visibility }), "Display")}
      {choices<rc.RemoteControlDisplayAutoOff>(rc.RemoteControlDisplayAutoOff, (autoOff) => new rc.RemoteControlRequest.SetDisplayAutoOff({ autoOff }), "Display auto-off")}
      {choices<rc.RemoteControlEspRadioMode>(rc.RemoteControlEspRadioMode, (mode) => new rc.RemoteControlRequest.SetEspRadioMode({ mode }), "Wireless mode")}
      {changeButton("Review pause radios", new rc.RemoteControlRequest.SleepRadios())}
      {changeButton("Review resume radios", new rc.RemoteControlRequest.WakeRadios())}
      {changeButton("Review announce node", new rc.RemoteControlRequest.AnnounceSelf())}
      <Wifi supported={description?.availableRequests ?? { kinds: [] }} status={wifi} disabled={!active || reading} blocked={uncertain} review={review} refresh={() => { void read(new rc.RemoteControlRequest.InspectWifiTransaction()); }} />
    </View>
    <View style={tab === "Access" ? styles.section : styles.hidden}><Controllers supported={description?.availableRequests ?? { kinds: [] }} inventory={controllers} disabled={!active || reading} blocked={uncertain} read={(request) => { void read(request); }} review={review} /></View>
    {pending !== undefined && <View style={styles.card}>
      <Text style={styles.title}>Review remote change</Text><RequestReview request={pending} />
      <Text>The node may apply this even if the reply is lost. A local cancellation cannot undo a submitted action.</Text>
      <Button title="Keep editing" onPress={() => setPending(undefined)} />
      <Button title="Apply reviewed change" disabled={submission.current || uncertain || !active || reading} onPress={submit} />
    </View>}
    {response !== undefined && <View style={styles.card}><Text style={styles.title}>Last read result</Text><Facts value={response} /></View>}
  </View>;
}
