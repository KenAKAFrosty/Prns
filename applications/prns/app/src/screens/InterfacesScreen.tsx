import { useCallback, useEffect, useRef, useState } from "react";
import { Link } from "expo-router";
import { View } from "react-native";
import * as platform from "@prns/app-platform";
import type { BluetoothDiscoveryGroups, PlatformStatus } from "@prns/app-platform";
import { prns_service_runtime as runtime } from "@prns/app-bindings";
import { Connections } from "../connections/Connections";
import { useRouteActive } from "../navigation";
import { useInstallation } from "../runtime/InstallationProvider";
import { Actions, Button, PageHeader, Panel, Screen, Text, TextInput } from "../ui/primitives";

export function InterfacesScreen() {
  const installation = useInstallation();
  const active = useRouteActive();
  const [status, setStatus] = useState<PlatformStatus>();
  const [groups, setGroups] = useState<BluetoothDiscoveryGroups>();
  const [draft, setDraft] = useState<string>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const mutating = useRef(false);
  const epoch = useRef(0);
  const refresh = useCallback(async () => {
    const ticket = ++epoch.current;
    const [next, saved] = await Promise.all([platform.status(), platform.bluetoothDiscoveryGroups()]);
    if (ticket === epoch.current) { setStatus(next); setGroups(saved); }
  }, []);
  useEffect(() => {
    if (!active) return;
    let reading = false;
    const read = async () => {
      if (reading || mutating.current) return;
      reading = true;
      try { await refresh(); } catch (cause) { setError(String(cause)); } finally { reading = false; }
    };
    void read();
    const timer = setInterval(() => { void read(); }, 2000);
    return () => { epoch.current++; clearInterval(timer); };
  }, [active, refresh]);
  const run = async (action: () => Promise<unknown>) => {
    if (!active || mutating.current) return;
    mutating.current = true; epoch.current++; setBusy(true); setError(undefined);
    try { await action(); await refresh(); }
    catch (cause) {
      setError(String(cause));
      // A rejected update may already be durable. Read before offering another edit.
      try { await refresh(); } catch { setGroups(undefined); }
    } finally { mutating.current = false; setBusy(false); }
  };
  const bluetooth = status?.bluetooth;
  return <Screen><PageHeader title="Connections" help={{ label: "About connections", content: <Text>Connect using your local network, a TCP server or Bluetooth. Stopping networking leaves your identities, contacts and saved history on this phone.</Text> }} />
    {error !== undefined && <Text accessibilityRole="alert">{error}</Text>}
    <Panel><Text accessibilityRole="header">Networking on this phone</Text>
      {installation.network !== undefined && <Text>{runtime.ServiceRuntimeNetworkPhase[installation.network.phase]}</Text>}
      {installation.owner === undefined ? <Link href="/setup" asChild><Button title="Open phone setup" /></Link> : <Actions>
        {installation.network?.phase === runtime.ServiceRuntimeNetworkPhase.Ready
          ? <Button title="Stop networking" disabled={installation.busy !== undefined} onPress={() => { void installation.stop(); }} />
          : <Button tone="primary" title="Start networking" disabled={installation.busy !== undefined} onPress={() => { void installation.start("open", bluetooth?.selected ?? installation.bluetooth); }} />}
      </Actions>}
    </Panel>
    <Panel><Text accessibilityRole="header">Bluetooth</Text>
      <Text>{bluetooth === undefined ? "Checking Bluetooth…" : `Permission: ${bluetooth.authorization}. Radio: ${bluetooth.availability}.`}</Text>
      <Text>{bluetooth?.attached ? "Bluetooth is attached to this phone’s node." : "Bluetooth is not currently attached."}</Text>
      {bluetooth?.authorization === "notDetermined" && <Button title="Allow Bluetooth" disabled={busy} onPress={() => { void run(platform.requestBluetoothPermission); }} />}
      {(bluetooth?.authorization === "denied" || bluetooth?.authorization === "restricted") && <>
        <Text>Allow Bluetooth in system settings to find nearby phones and nodes. Saved data and other connection types remain available.</Text>
        <Button title="Open app settings" disabled={busy} onPress={() => { void run(() => platform.openSettings("application")); }} />
      </>}
      {bluetooth?.availability === "off" && <Button title="Open Bluetooth settings" disabled={busy} onPress={() => { void run(() => platform.openSettings("bluetooth")); }} />}
      {bluetooth?.availability === "locationOff" && <><Text>This Android version requires location services for nearby Bluetooth discovery.</Text>
        <Button title="Open location settings" disabled={busy} onPress={() => { void run(() => platform.openSettings("location")); }} /></>}
      {bluetooth !== undefined && <Button title={bluetooth.selected ? "Turn off Bluetooth connections" : "Use Bluetooth connections"} disabled={busy || bluetooth.availability === "unsupported"}
        onPress={() => { void run(() => platform.setBluetoothEnabled(!bluetooth.selected)); }} />}
      <Text>Discovery groups determine which nearby PRNS Bluetooth peers this phone discovers. They are independent of chat groups.</Text>
      {groups !== undefined && <>
        <Text>Saved groups: {groups.saved.join(", ")}</Text><Text>{groups.applied === null ? "The saved groups are not confirmed active." : `Active groups: ${groups.applied.join(", ")}`}</Text>
        {draft === undefined ? <Button title="Change discovery groups" disabled={busy} onPress={() => setDraft(groups.saved.join("\n"))} /> : <>
          <TextInput accessibilityLabel="Bluetooth discovery groups" multiline value={draft} onChangeText={setDraft} editable={!busy} />
          <Text>Enter one group name per line. Changing groups can disconnect peers using the previous settings.</Text>
          <Button title="Save discovery groups" disabled={busy} onPress={() => { const proposed = draft.split(/\r?\n/).map((item) => item.trim()).filter(Boolean); void run(async () => { await platform.setBluetoothDiscoveryGroups(proposed); setDraft(undefined); }); }} />
          <Button title="Keep saved discovery groups" disabled={busy} onPress={() => setDraft(undefined)} />
        </>}
      </>}

    </Panel>
    {status !== undefined && status.notifications !== "notApplicable" && <Panel>
      <Text accessibilityRole="header">Running-node notification</Text><Text>Status: {status.notifications}</Text>
      <Text>Android can show an ongoing notification with a Stop control while this phone’s node runs. This does not enable message alerts.</Text>
      {status.notifications !== "enabled" && <Button title={status.notifications === "blocked" ? "Open notification settings" : "Allow running-node notification"} disabled={busy}
        onPress={() => { void run(() => status.notifications === "blocked" ? platform.openSettings("notifications") : platform.requestNotificationPermission()); }} />}
    </Panel>}
    <Connections owner={installation.owner} active={active} networkKey={installation.networkKey} />
  </Screen>;
}
