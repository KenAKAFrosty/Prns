import { Button, Text, TextInput } from "../ui/primitives";
import { useState } from "react";
import { View } from "react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { LoRa } from "./LoRa";
import { Facts, label } from "./presentation";

export function Interface({ entry, supported, configuration, discovery, peers, radio, disabled, blocked, read, review }: {
  entry: rc.RemoteControlInterfaceEntry; supported: rc.RemoteControlRequestSet;
  configuration: rc.RemoteControlInterfaceConfigOutcome | undefined; discovery: rc.RemoteControlDiscoveryGroupsInventoryOutcome | undefined;
  peers: rc.RemoteControlInterfacePeersOutcome | undefined; radio: rc.RemoteControlRadioStatus | undefined;
  disabled: boolean; blocked: boolean; read(request: rc.RemoteControlRequest): void; review(request: rc.RemoteControlRequest): void;
}) {
  const styles = useStyles();
  const [editing, setEditing] = useState(false);
  const [group, setGroup] = useState("");
  const [groups, setGroups] = useState("");
  const [ssid, setSsid] = useState("");
  const [password, setPassword] = useState("");
  const can = (kind: rc.RemoteControlRequestKind) => supported.kinds.includes(kind);
  const missing = configuration?.tag === rc.RemoteControlInterfaceConfigOutcome_Tags.UnknownInterface;
  const id = entry.id;
  const identifier = hex(id.value);
  const blockedChange = disabled || blocked || missing;
  const card = configuration?.tag === rc.RemoteControlInterfaceConfigOutcome_Tags.Card ? configuration.inner.value : undefined;
  const peerPage = peers?.tag === rc.RemoteControlInterfacePeersOutcome_Tags.Page ? peers.inner.value : undefined;
  const nextPeers = peerPage?.continuation.tag === rc.RemoteControlPeerContinuation_Tags.More ? new rc.RemoteControlRequest.InventoryInterfacePeers({ id, page: new rc.RemoteControlPeerPage.After({ value: peerPage.continuation.inner.value }) }) : undefined;
  const savedRadio = radio?.tag === rc.RemoteControlRadioStatus_Tags.Status && radio.inner.saved.tag === rc.RemoteControlRadioSaved_Tags.Confirmed ? radio.inner.saved.inner.value : undefined;
  const reportedProfile = savedRadio?.tag === rc.RemoteControlRadioConfiguration_Tags.Profile ? savedRadio.inner.value : card?.config ? { value: card.config } : undefined;
  return <View style={styles.card}>
    <Text style={styles.title}>{card?.name || `Interface ${identifier}`}: {label(rc.RemoteControlInterfaceKind[entry.kind])}</Text>
    <Text>{label(rc.RemoteControlInterfaceMode[entry.mode])} · {entry.enabled ? "Enabled" : "Disabled"} · received {entry.rxBytes.toString()} bytes · sent {entry.txBytes.toString()} bytes</Text>
    {missing && <Text accessibilityRole="alert">The node no longer has this interface. Refresh its interface inventory.</Text>}
    {can(rc.RemoteControlRequestKind.InventoryInterfaceConfig) && <Button title={`Read interface ${identifier} configuration`} disabled={disabled} onPress={() => read(new rc.RemoteControlRequest.InventoryInterfaceConfig({ id }))} />}
    {can(rc.RemoteControlRequestKind.InventoryInterfacePeers) && <Button title={`Read interface ${identifier} peers`} disabled={disabled} onPress={() => read(new rc.RemoteControlRequest.InventoryInterfacePeers({ id, page: new rc.RemoteControlPeerPage.First() }))} />}
    {can(rc.RemoteControlRequestKind.InventoryInterfaceDiscoveryGroups) && <Button title={`Read interface ${identifier} discovery groups`} disabled={disabled} onPress={() => read(new rc.RemoteControlRequest.InventoryInterfaceDiscoveryGroups({ id }))} />}
    {can(rc.RemoteControlRequestKind.InspectRadio) && <Button title={`Read interface ${identifier} radio`} disabled={disabled} onPress={() => read(new rc.RemoteControlRequest.InspectRadio({ id }))} />}
    {configuration !== undefined && <Facts value={configuration} />}
    {discovery !== undefined && <Facts value={discovery} />}
    {radio !== undefined && <Facts value={radio} />}
    {peerPage !== undefined && <>
      <Facts value={peerPage} />
      {nextPeers !== undefined && <Button title={`Read next peer page for ${identifier}`} disabled={disabled} onPress={() => read(nextPeers)} />}
    </>}
    <Button title={`${editing ? "Hide" : "Edit"} interface ${identifier} settings`} onPress={() => setEditing((value) => !value)} disabled={missing} />
    <View style={editing ? styles.section : styles.hidden}>
      <Text>Changes can disconnect this node. Keep another way to reach it. Read settings after a change; the node validates its supported values.</Text>
      {can(rc.RemoteControlRequestKind.SetInterfacePower) && <View style={styles.row}>{Object.entries(rc.RemoteControlInterfacePower).filter(([, value]) => typeof value === "number").map(([name, value]) => <Button key={name} title={`Interface power: ${label(name)}`} disabled={blockedChange} onPress={() => review(new rc.RemoteControlRequest.SetInterfacePower({ id, power: value as rc.RemoteControlInterfacePower }))} />)}</View>}
      {can(rc.RemoteControlRequestKind.SetInterfaceMode) && <View style={styles.row}>{Object.entries(rc.RemoteControlInterfaceMode).filter(([, value]) => typeof value === "number").map(([name, value]) => <Button key={name} title={`Interface mode: ${label(name)}`} disabled={blockedChange} onPress={() => review(new rc.RemoteControlRequest.SetInterfaceMode({ id, mode: value as rc.RemoteControlInterfaceMode }))} />)}</View>}
      {can(rc.RemoteControlRequestKind.SetStationUplink) && <View style={styles.row}>{Object.entries(rc.RemoteControlStationUplink).filter(([, value]) => typeof value === "number").map(([name, value]) => <Button key={name} title={`Saved Wi-Fi uplink: ${label(name)}`} disabled={blockedChange} onPress={() => review(new rc.RemoteControlRequest.SetStationUplink({ id, uplink: value as rc.RemoteControlStationUplink }))} />)}</View>}
      {can(rc.RemoteControlRequestKind.SetInterfaceGroup) && <>
        <TextInput accessibilityLabel={`Interface ${identifier} group`} value={group} onChangeText={setGroup} autoCapitalize="none" autoCorrect={false} style={styles.input} />
        <Button title="Review interface group" disabled={blockedChange} onPress={() => review(new rc.RemoteControlRequest.SetInterfaceGroup({ id, group: { value: group } }))} />
      </>}
      {can(rc.RemoteControlRequestKind.ReplaceInterfaceDiscoveryGroups) && discovery?.tag === rc.RemoteControlDiscoveryGroupsInventoryOutcome_Tags.Groups && <>
        <Text>Replace the complete discovery-group list. Enter one group per line; an empty list removes all groups.</Text>
        <Button title="Copy current discovery groups into editor" disabled={blockedChange} onPress={() => setGroups(discovery.inner.value.groups.join("\n"))} />
        <TextInput accessibilityLabel={`Interface ${identifier} discovery groups`} value={groups} onChangeText={setGroups} multiline autoCapitalize="none" autoCorrect={false} style={styles.input} />
        <Button title="Review replacing discovery groups" disabled={blockedChange} onPress={() => review(new rc.RemoteControlRequest.ReplaceInterfaceDiscoveryGroups({ id, groups: { groups: groups.split("\n").map((value) => value.trim()).filter(Boolean) } }))} />
      </>}
      {can(rc.RemoteControlRequestKind.SetInterfaceWifiStation) && <>
        <Text>This interface-specific update directly changes its saved network. Use staged Wi-Fi setup when the node provides it.</Text>
        <TextInput accessibilityLabel={`Interface ${identifier} Wi-Fi name`} value={ssid} onChangeText={setSsid} autoCapitalize="none" autoCorrect={false} style={styles.input} />
        <TextInput accessibilityLabel={`Interface ${identifier} Wi-Fi password`} value={password} onChangeText={setPassword} secureTextEntry autoCapitalize="none" autoCorrect={false} style={styles.input} />
        <Button title="Review interface Wi-Fi credentials" disabled={blockedChange || !ssid} onPress={() => review(new rc.RemoteControlRequest.SetInterfaceWifiStation({ id, station: { ssid, password } }))} />
      </>}
      {(entry.kind === rc.RemoteControlInterfaceKind.LoRa || radio?.tag === rc.RemoteControlRadioStatus_Tags.Status) && <LoRa id={id} radio={radio} reported={reportedProfile} supported={supported} disabled={blockedChange} review={review} />}
      {can(rc.RemoteControlRequestKind.ConfigureRadio) && radio?.tag === rc.RemoteControlRadioStatus_Tags.Status && <Button title="Review clearing saved radio configuration" disabled={blockedChange} onPress={() => review(new rc.RemoteControlRequest.ConfigureRadio({ id, configuration: new rc.RemoteControlRadioConfiguration.Unconfigured() }))} />}
    </View>
  </View>;
}
