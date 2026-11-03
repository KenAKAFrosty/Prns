import { Button, Text, TextInput } from "../ui/primitives";
import { useEffect, useRef, useState } from "react";
import { Share, Switch, View } from "react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { detail, Permissions } from "./presentation";

export function Access({ client, active, changed }: { client: rc.HostClientHandleLike; active: boolean; changed(): void }) {
  const styles = useStyles();
  const liveClient = useRef(client); liveClient.current = client;
  const [controllerClient, setControllerClient] = useState<rc.HostClientHandleLike>();
  const [storedController, setController] = useState<rc.RemoteControlControllerIdentity>();
  const [exported, setExported] = useState<string>();
  const [controllerError, setControllerError] = useState<string>();
  const [controllerLoading, setControllerLoading] = useState(false);
  const [controllerRevision, setControllerRevision] = useState(0);
  const [input, setInput] = useState("");
  const [card, setCard] = useState<rc.RemoteControlTargetAccessCard>();
  const [trusted, setTrusted] = useState(false);
  const saving = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const controller = controllerClient === client ? storedController : undefined;
  useEffect(() => { setController(undefined); setControllerClient(undefined); setExported(undefined); setControllerError(undefined); setControllerLoading(false); setTrusted(false); setError(undefined); saving.current = false; setBusy(false); }, [client]);
  useEffect(() => {
    if (!active) return;
    const abort = new AbortController();
    setControllerError(undefined); setControllerLoading(true);
    void client.remoteControlControllerIdentity({ signal: abort.signal }).then((result) => {
      if (abort.signal.aborted || liveClient.current !== client) return;
      if (result.tag === rc.RemoteControlControllerIdentitySettlement_Tags.Completed) {
        setControllerClient(client); setController(result.inner.value);
        const encoded = rc.encodeRemoteControlControllerCard({ controller: result.inner.value });
        if (encoded.tag === rc.RemoteControlEncodeRemoteControlControllerCardSettlement_Tags.Completed) setExported(encoded.inner.value);
        else setControllerError(detail(encoded.inner.failure));
      } else setControllerError(detail(result.inner.failure));
    }).catch((cause: unknown) => { if (!abort.signal.aborted && liveClient.current === client) setControllerError(detail(cause)); })
      .finally(() => { if (!abort.signal.aborted && liveClient.current === client) setControllerLoading(false); });
    return () => abort.abort();
  }, [active, client, controllerRevision]);
  function review() {
    setTrusted(false); setCard(undefined); setError(undefined);
    try {
      const decoded = rc.decodeRemoteControlTargetAccessCard(input.trim());
      if (decoded.tag === rc.RemoteControlDecodeRemoteControlTargetAccessCardSettlement_Tags.Failed) throw decoded.inner.failure;
      setCard(decoded.inner.value);
    } catch (cause) { setError(detail(cause)); }
  }
  async function save() {
    if (!active || card === undefined || !trusted || !recipientMatches || busy || saving.current) return;
    saving.current = true;
    setBusy(true); setError(undefined);
    try {
      const result = await client.saveRemoteControlTargetAccessCard(card);
      if (liveClient.current !== client) return;
      if (result.tag === rc.RemoteControlSaveRemoteControlTargetAccessCardSettlement_Tags.Failed) throw result.inner.failure;
      setCard(undefined); setInput(""); setTrusted(false); changed();
    } catch (cause) { if (liveClient.current === client) setError(`Access was not confirmed saved: ${detail(cause)}. Existing access is not replaced automatically.`); }
    finally { if (liveClient.current === client) { saving.current = false; setBusy(false); } }
  }
  const localHash = controller === undefined ? undefined : hex(rc.remoteControlControllerIdentityHash(controller).value);
  const recipientHash = card === undefined ? undefined : hex(rc.remoteControlControllerIdentityHash(card.recipient).value);
  const recipientMatches = controller !== undefined && card !== undefined && hex(controller.publicKeys) === hex(card.recipient.publicKeys);
  return <View style={styles.section}>
    <Text style={styles.heading}>Controller identity</Text>
    <Text>This phone uses a separate identity to manage nodes. Share its card so a node administrator can grant access; sharing the card alone does not grant access.</Text>
    {localHash !== undefined && <Text selectable style={styles.fingerprint}>{localHash}</Text>}
    {exported !== undefined && <><Text selectable>{exported}</Text><Button title="Share controller card" onPress={() => { void Share.share({ message: exported }).catch((cause: unknown) => setError(detail(cause))); }} /></>}
    {controllerError !== undefined && <>
      <Text accessibilityRole="alert" style={styles.error}>Controller card could not be loaded: {controllerError}</Text>
      <Button title="Retry loading controller card" disabled={!active || controllerLoading} onPress={() => {
        if (active && !controllerLoading && liveClient.current === client) setControllerRevision((value) => value + 1);
      }} />
    </>}
    <Text style={styles.heading}>Import node access</Text>
    <Text>Paste a target-access card from a trusted node administrator. A valid card is public access information, not proof that the node has granted permission.</Text>
    <TextInput accessibilityLabel="Node access card" value={input} onChangeText={(value) => { setInput(value); setCard(undefined); setTrusted(false); }} multiline autoCapitalize="none" autoCorrect={false} style={styles.input} editable={!busy} />
    <Button title="Review access card" onPress={review} disabled={!input.trim() || busy} />
    {card !== undefined && <View style={styles.card}>
      <Text>Node identity</Text><Text selectable style={styles.fingerprint}>{hex(rc.remoteControlTargetIdentityHash(card.access.target).value)}</Text>
      <Text>Recipient controller</Text><Text selectable style={styles.fingerprint}>{recipientHash}</Text>
      <Text>{recipientMatches ? "Recipient matches this installation." : "This card belongs to a different controller and cannot be imported here."}</Text>
      <Permissions authority={card.access.authority} permittedRequests={card.access.permittedRequests} />
      <View style={styles.row}><Switch accessibilityLabel="I trust the source and reviewed the exact access" value={trusted} onValueChange={setTrusted} disabled={!recipientMatches || busy} /><Text>I trust the source and reviewed this access.</Text></View>
      <Button title="Save reviewed node access" disabled={!active || !trusted || !recipientMatches || busy} onPress={() => { void save(); }} />
    </View>}
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
  </View>;
}
