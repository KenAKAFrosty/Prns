import { Button, Text, TextInput } from "../ui/primitives";
import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, Switch, View } from "react-native";
import { prns_chat_app as app, prns_host_uniffi as host } from "@prns/app-bindings";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { errorText, sameGuard, sameProfileId, sameValue } from "./values";

type Editor = {
  base: host.ConnectionSnapshot;
  id: host.ConnectionProfile["id"];
  existing: boolean;
  label: string;
  target: string;
  enabled: boolean;
};
type Save = {
  base: host.ConnectionSnapshot;
  profiles: host.ConnectionProfile[];
  description: string;
  editor: boolean;
};

/** Saved settings outlive a host generation. Only live observations are replaced
 * when the host changes; draft guards and uncertain proposals remain unchanged.
 */
export function Connections({ owner, active, networkKey }: { owner?: owners.ServiceRuntimeLike | undefined; active: boolean; networkKey?: string | undefined }) {
  const styles = useStyles();
  const [client, setClient] = useState<host.ConnectionProfileClientLike>();
  const [saved, setSaved] = useState<host.ConnectionSnapshot>();
  const [editor, setEditor] = useState<Editor>();
  const [pending, setPending] = useState<Save>();
  const [checkedPending, setCheckedPending] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string>();
  const [error, setError] = useState<string>();
  const [refresh, setRefresh] = useState(0);
  const [binding, setBinding] = useState<host.ConnectionBindingClientLike>();
  const [live, setLive] = useState<host.ConnectionBindingSnapshot>();
  const [bindingStatus, setBindingStatus] = useState<host.ConnectionBindingStatus>();
  const [network, setNetwork] = useState<host.HostSnapshot>();
  const [liveError, setLiveError] = useState<string>();
  const busyRef = useRef(false);
  const pendingRef = useRef<Save | undefined>(undefined);
  const readEpoch = useRef(0);
  const bindingRef = useRef<host.ConnectionBindingClientLike | undefined>(undefined);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; readEpoch.current += 1; }; }, []);

  useEffect(() => {
    if (client !== undefined) return;
    try { setClient(app.appConnectionProfiles()); setError(undefined); }
    catch (cause) { setError(`Saved connections are not available yet. ${errorText(cause)}`); }
  }, [client, owner, refresh]);

  useEffect(() => {
    if (!active || client === undefined) return;
    let disposed = false;
    let reading = false;
    const read = async () => {
      if (reading || busyRef.current || pendingRef.current !== undefined) return;
      reading = true;
      const epoch = ++readEpoch.current;
      try {
        const value = await client.snapshot();
        if (!disposed && mounted.current && epoch === readEpoch.current) { setSaved(value); setError(undefined); }
      } catch (cause) {
        if (!disposed && mounted.current && epoch === readEpoch.current) setError(errorText(cause));
      } finally { reading = false; }
    };
    void read();
    const timer = setInterval(() => { void read(); }, 2000);
    return () => { disposed = true; clearInterval(timer); readEpoch.current += 1; };
  }, [active, client, refresh]);

  useEffect(() => {
    bindingRef.current = undefined;
    setBinding(undefined); setLive(undefined); setBindingStatus(undefined); setNetwork(undefined); setLiveError(undefined);
    if (!active || owner === undefined) return;
    let current: host.ConnectionBindingClientLike;
    let currentHost: ReturnType<owners.ServiceRuntimeLike["host"]>;
    try { current = owner.connectionBinding(); currentHost = owner.host(); bindingRef.current = current; setBinding(current); }
    catch (cause) { setLiveError(errorText(cause)); return; }
    const abort = new AbortController();
    let reading = false;
    const read = async () => {
      if (reading) return;
      reading = true;
      try {
        const status = current.status();
        if (!abort.signal.aborted) setBindingStatus(status);
        const [value, health] = await Promise.allSettled([current.snapshot({ signal: abort.signal }), currentHost.snapshot({ signal: abort.signal })]);
        if (!abort.signal.aborted) {
          setLive(value.status === "fulfilled" ? value.value : undefined);
          setNetwork(health.status === "fulfilled" ? health.value : undefined);
          setLiveError(value.status === "rejected" ? errorText(value.reason) : health.status === "rejected" ? `Network health is unavailable. ${errorText(health.reason)}` : undefined);
        }
      } catch (cause) { if (!abort.signal.aborted) setLiveError(errorText(cause)); }
      finally { reading = false; }
    };
    void read();
    const timer = setInterval(() => { void read(); }, 2000);
    return () => { abort.abort(); clearInterval(timer); if (bindingRef.current === current) bindingRef.current = undefined; };
  }, [active, owner, networkKey, refresh]);

  const locked = busy || pending !== undefined;
  const staleEditor = editor !== undefined && saved !== undefined && !sameGuard(editor.base.guard, saved.guard);
  const existingEditorRemoved = editor?.existing === true && saved !== undefined && !saved.profiles.some((profile) => sameProfileId(profile.id, editor.id));
  const newEditorAlreadyExists = editor?.existing === false && saved !== undefined && saved.profiles.some((profile) => sameProfileId(profile.id, editor.id));
  const editorUnavailable = existingEditorRemoved || newEditorAlreadyExists;

  const reconcile = async (attempt: Save) => {
    if (client === undefined) return;
    const epoch = ++readEpoch.current;
    try {
      const value = await client.snapshot();
      if (!mounted.current || pendingRef.current !== attempt || epoch !== readEpoch.current) return;
      setSaved(value); setCheckedPending(true);
      if (hex(value.guard.incarnation) === hex(attempt.base.guard.incarnation) && value.guard.revision >= attempt.base.guard.revision && sameValue(value.profiles, attempt.profiles)) {
        pendingRef.current = undefined; setPending(undefined); setCheckedPending(false);
        if (attempt.editor) setEditor(undefined);
        setMessage("These settings are saved. No repeat save was sent."); setError(undefined);
      } else {
        setMessage(sameGuard(value.guard, attempt.base.guard)
          ? "The saved settings are unchanged. Your change has been kept for review."
          : "Saved settings changed. Your change has been kept for review.");
      }
    } catch (cause) {
      if (mounted.current && pendingRef.current === attempt && epoch === readEpoch.current) {
        setCheckedPending(false); setError(`The save outcome is still unknown. Check the saved settings before making another change. ${errorText(cause)}`);
      }
    }
  };

  const save = async (attempt: Save) => {
    if (client === undefined || busyRef.current || pendingRef.current !== undefined) return;
    busyRef.current = true; setBusy(true); setError(undefined); setMessage(undefined);
    ++readEpoch.current;
    pendingRef.current = attempt; setPending(attempt); setCheckedPending(false);
    try {
      const result = await client.replace(attempt.base.guard, attempt.profiles);
      if (!mounted.current || pendingRef.current !== attempt) return;
      ++readEpoch.current; setSaved(result);
      pendingRef.current = undefined; setPending(undefined);
      if (attempt.editor) setEditor(undefined);
      setMessage("Saved. Networking will use these settings when it is running.");
      setRefresh((value) => value + 1);
    } catch (cause) {
      if (!mounted.current) return;
      setError(`Could not confirm the save. ${errorText(cause)}`);
      await reconcile(attempt);
    } finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  };

  const edit = (profile?: host.ConnectionProfile) => {
    if (saved === undefined || busyRef.current || pendingRef.current !== undefined) return;
    try {
      if (profile !== undefined && !host.InterfaceConfig.TcpClient.instanceOf(profile.config)) return;
      setEditor({ base: saved, id: profile?.id ?? host.newConnectionProfileId(), existing: profile !== undefined,
        label: profile?.label ?? "", target: profile !== undefined && host.InterfaceConfig.TcpClient.instanceOf(profile.config) ? profile.config.inner.target : "",
        enabled: profile?.enabled ?? true });
      setError(undefined); setMessage(undefined);
    } catch (cause) { setError(errorText(cause)); }
  };
  const saveEditor = () => {
    if (editor === undefined || staleEditor || locked || editorUnavailable) return;
    try {
      const previous = editor.base.profiles.find((profile) => sameProfileId(profile.id, editor.id));
      const bitrate = previous !== undefined && host.InterfaceConfig.TcpClient.instanceOf(previous.config) ? previous.config.inner.bitrate : new host.Bitrate.Auto();
      const profile: host.ConnectionProfile = { id: editor.id, label: editor.label.trim(), enabled: editor.enabled,
        config: host.connectionTcpClientConfigFromTarget(editor.target, bitrate), routing: previous?.routing };
      const profiles = editor.existing ? editor.base.profiles.map((value) => sameProfileId(value.id, editor.id) ? profile : value) : [...editor.base.profiles, profile];
      void save({ base: editor.base, profiles, description: `Save ${profile.label || "server"}`, editor: true });
    } catch (cause) { setError(errorText(cause)); }
  };
  const addLan = () => {
    if (saved === undefined || busyRef.current || pendingRef.current !== undefined || editor !== undefined) return;
    try {
      const profile: host.ConnectionProfile = { id: host.newConnectionProfileId(), label: "Local network", enabled: false,
        config: new host.InterfaceConfig.AutoLan({ devices: [], ignoredDevices: [] }), routing: undefined };
      void save({ base: saved, profiles: [...saved.profiles, profile], description: "Add local discovery", editor: false });
    } catch (cause) { setError(errorText(cause)); }
  };
  const changeEnabled = (profile: host.ConnectionProfile, enabled: boolean) => {
    if (saved === undefined || locked || editor !== undefined) return;
    void save({ base: saved, profiles: saved.profiles.map((value) => sameProfileId(value.id, profile.id) ? { ...value, enabled } : value), description: `${enabled ? "Enable" : "Disable"} ${profile.label}`, editor: false });
  };
  const remove = (profile: host.ConnectionProfile) => {
    if (saved === undefined || locked || editor !== undefined) return;
    void save({ base: saved, profiles: saved.profiles.filter((value) => !sameProfileId(value.id, profile.id)), description: `Remove ${profile.label}`, editor: false });
  };
  const retry = async (profile: host.ConnectionProfile) => {
    if (binding === undefined || saved === undefined || busyRef.current || pendingRef.current !== undefined) return;
    busyRef.current = true; setBusy(true); setError(undefined);
    try { await binding.retry(saved.guard, profile.id); if (mounted.current && bindingRef.current === binding) setMessage("Retry requested. Connection health is shown separately below."); }
    catch (cause) { if (mounted.current && bindingRef.current === binding) setError(errorText(cause)); }
    finally { busyRef.current = false; if (mounted.current) { setBusy(false); if (bindingRef.current === binding) setRefresh((value) => value + 1); } }
  };
  const review = () => {
    if (saved === undefined || busyRef.current || (pending !== undefined && !checkedPending) || editorUnavailable) return;
    if (editor !== undefined) setEditor({ ...editor, base: saved });
    pendingRef.current = undefined; setPending(undefined); setCheckedPending(false); setError(undefined);
    setMessage("Current saved settings loaded. Review your draft, then save when ready.");
  };

  const applied = saved !== undefined && live !== undefined && live.phase === host.ConnectionBindingPhase.Active && sameGuard(saved.guard, live.saved) && live.appliedRevision === saved.guard.revision;
  return <View style={styles.section}>
    <Text accessibilityRole="header" style={styles.heading}>Saved connections</Text>
    <Text>Choose local discovery and TCP servers. Settings remain available when networking is stopped.</Text>
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
    {message !== undefined && <Text accessibilityLiveRegion="polite">{message}</Text>}
    {pending === undefined && (error !== undefined || (networkKey !== undefined && liveError !== undefined)) &&
      <Button title="Retry loading connections" disabled={busy || !active} onPress={() => setRefresh((value) => value + 1)} />}
    {saved === undefined && error === undefined && <ActivityIndicator accessibilityLabel="Loading saved connections" />}
    {saved !== undefined && <>
      {saved.profiles.length === 0 && <Text>No saved connections.</Text>}
      {!saved.profiles.some((profile) => host.InterfaceConfig.AutoLan.instanceOf(profile.config)) && <View style={styles.section}>
        <Text>Local network discovery is not configured.</Text>
        <Button title="Add local discovery" disabled={locked || editor !== undefined} onPress={addLan} />
      </View>}
      {saved.profiles.map((profile) => <View key={hex(profile.id)} style={styles.card}>
        <Text style={styles.title}>{profile.label}</Text>
        {host.InterfaceConfig.AutoLan.instanceOf(profile.config) && <Text>Local network discovery</Text>}
        {host.InterfaceConfig.TcpClient.instanceOf(profile.config) && <Text selectable>{profile.config.inner.target}</Text>}
        <View style={styles.row}><Text>{profile.enabled ? "Enabled" : "Disabled"}</Text><Switch accessibilityLabel={`Enable ${profile.label}`} value={profile.enabled} disabled={locked || editor !== undefined} onValueChange={(enabled) => changeEnabled(profile, enabled)} /></View>
        {host.InterfaceConfig.TcpClient.instanceOf(profile.config) && <View style={styles.row}>
          <Button title={`Edit ${profile.label}`} disabled={locked || editor !== undefined} onPress={() => edit(profile)} />
          <Button title={`Remove ${profile.label}`} disabled={locked || editor !== undefined} onPress={() => remove(profile)} />
        </View>}
        {profile.enabled && binding !== undefined && <Button title={`Retry ${profile.label}`} disabled={locked || editor !== undefined || bindingStatus?.phase !== host.ConnectionBindingPhase.Active} onPress={() => { void retry(profile); }} />}
      </View>)}
      <Button title="Add TCP server" disabled={locked || editor !== undefined} onPress={() => edit()} />
    </>}
    {editor !== undefined && <View style={styles.card}>
      <Text accessibilityRole="header" style={styles.title}>{editor.existing ? "Edit TCP server" : "New TCP server"}</Text>
      <TextInput accessibilityLabel="Connection name" placeholder="Connection name" style={styles.input} value={editor.label} editable={!locked} onChangeText={(label) => { if (!busyRef.current && pendingRef.current === undefined) setEditor((value) => value === undefined ? value : { ...value, label }); }} />
      <TextInput accessibilityLabel="Server address" placeholder="node.example:4242" autoCapitalize="none" autoCorrect={false} style={styles.input} value={editor.target} editable={!locked} onChangeText={(target) => { if (!busyRef.current && pendingRef.current === undefined) setEditor((value) => value === undefined ? value : { ...value, target }); }} />
      <Text style={styles.muted}>Enter the server address and port. IPv6 addresses use brackets.</Text>
      <View style={styles.row}><Text>Enable this server</Text><Switch accessibilityLabel="Enable edited server" value={editor.enabled} disabled={locked} onValueChange={(enabled) => { if (!busyRef.current && pendingRef.current === undefined) setEditor((value) => value === undefined ? value : { ...value, enabled }); }} /></View>
      {staleEditor && <Text accessibilityRole="alert">Saved settings changed while you were editing. Your draft is unchanged.</Text>}
      {existingEditorRemoved && <Text accessibilityRole="alert">This connection was removed elsewhere. Cancel this edit to add a new server.</Text>}
      {newEditorAlreadyExists && <Text accessibilityRole="alert">This server is already saved. Cancel this draft to edit the saved connection.</Text>}
      <Button title="Save server" disabled={locked || staleEditor || editorUnavailable} onPress={saveEditor} />
      {staleEditor && pending === undefined && <Button title="Review latest settings" disabled={busy || editorUnavailable} onPress={review} />}
      <Button title="Cancel server edit" disabled={busy || (pending !== undefined && !checkedPending)} onPress={() => { if (busyRef.current || (pendingRef.current !== undefined && !checkedPending)) return; setEditor(undefined); pendingRef.current = undefined; setPending(undefined); setCheckedPending(false); setError(undefined); }} />
    </View>}
    {pending !== undefined && <View style={styles.card}>
      <Text>Unconfirmed change: {pending.description}</Text>
      {busy ? <ActivityIndicator accessibilityLabel="Saving connections" /> : <>
        <Button title="Check saved settings" onPress={() => { void reconcile(pending); }} />
        {checkedPending && <Button title="Review current settings" disabled={editorUnavailable} onPress={review} />}
      </>}
    </View>}
    <View style={styles.card}>
      <Text accessibilityRole="header" style={styles.title}>Networking</Text>
      {owner === undefined ? <Text>Saved connections will apply when networking starts.</Text> : <>
        <Text>{applied ? "Saved settings are active." : "Saved settings and active connections are shown separately."}</Text>
        {bindingStatus !== undefined && <Text>{bindingStatus.phase === host.ConnectionBindingPhase.Active ? "Running" : bindingStatus.phase === host.ConnectionBindingPhase.Fenced ? "Connection changes paused" : bindingStatus.phase === host.ConnectionBindingPhase.Closing ? "Disconnecting" : "Stopped"}</Text>}
        {bindingStatus?.cleanupFailure !== undefined && <Text accessibilityRole="alert">Connection cleanup needs attention: {errorText(bindingStatus.cleanupFailure)}</Text>}
        {liveError !== undefined && <Text accessibilityRole="alert">{liveError}</Text>}
        {live?.attachments.map((attachment) => {
          const health = attachment.interface === undefined ? undefined : network?.interfaces.find((value) => hex(value.interfaceId) === hex(attachment.interface!));
          return <View key={hex(attachment.profile.id)} style={styles.section}>
            <Text>{attachment.profile.label} · {host.ConnectionAttachmentState[attachment.state]}</Text>
            {attachment.failure !== undefined && <Text accessibilityRole="alert">{errorText(attachment.failure)}</Text>}
            <Text>{health === undefined ? "Connection health is not available yet." : `Network health: ${host.InterfaceHealth[health.health]}`}</Text>
          </View>;
        })}
        <Text style={styles.muted}>A saved connection can be active while its server is unreachable. Message delivery is shown in each conversation.</Text>
      </>}
    </View>
  </View>;
}
