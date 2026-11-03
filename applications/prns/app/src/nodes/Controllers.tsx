import { Button, Text, TextInput } from "../ui/primitives";
import { useState } from "react";
import { Switch, View } from "react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { detail, requestLabel } from "./presentation";

export function Controllers({ supported, inventory, disabled, blocked, read, review }: {
  supported: rc.RemoteControlRequestSet; inventory: rc.RemoteControlControllerInventory | undefined; disabled: boolean; blocked: boolean;
  read(request: rc.RemoteControlRequest): void; review(request: rc.RemoteControlRequest): void;
}) {
  const styles = useStyles();
  const [input, setInput] = useState("");
  const [card, setCard] = useState<rc.RemoteControlControllerCard>();
  const [permitted, setPermitted] = useState<rc.RemoteControlRequestKind[]>([]);
  const [trusted, setTrusted] = useState(false);
  const [error, setError] = useState<string>();
  const can = (kind: rc.RemoteControlRequestKind) => supported.kinds.includes(kind);
  const choices = supported.kinds.filter((kind) => !rc.remoteControlRequestRequiresAdministrator(kind));
  const chosen = permitted.filter((kind) => choices.includes(kind));
  function decode() {
    setCard(undefined); setTrusted(false); setPermitted([]); setError(undefined);
    try {
      const result = rc.decodeRemoteControlControllerCard(input.trim());
      if (result.tag === rc.RemoteControlDecodeRemoteControlControllerCardSettlement_Tags.Failed) throw result.inner.failure;
      setCard(result.inner.value);
    } catch (cause) { setError(detail(cause)); }
  }
  const nextPage = inventory?.continuation.tag === rc.RemoteControlControllerContinuation_Tags.More ? new rc.RemoteControlRequest.InventoryControllers({ page: new rc.RemoteControlControllerPage.After({ value: inventory.continuation.inner.value }) }) : undefined;
  return <View style={styles.section}>
    <Text style={styles.title}>Controller access</Text>
    <Text>These are the node’s saved controllers. Their permissions are not shown here. The node may prevent you from removing protected access.</Text>
    {can(rc.RemoteControlRequestKind.InventoryControllers) && <Button title="Read devices with node access" disabled={disabled} onPress={() => read(new rc.RemoteControlRequest.InventoryControllers({ page: new rc.RemoteControlControllerPage.First() }))} />}
    {inventory?.hashes.map((hash) => <View style={styles.card} key={hex(hash.value)}>
      <Text selectable style={styles.fingerprint}>{hex(hash.value)}</Text>
      {can(rc.RemoteControlRequestKind.RevokeController) && <Button title={`Review revoking ${hex(hash.value).slice(0, 12)}`} disabled={disabled || blocked} onPress={() => review(new rc.RemoteControlRequest.RevokeController({ hash }))} />}
    </View>)}
    {nextPage !== undefined && <Button title="Read next devices with access" disabled={disabled} onPress={() => read(nextPage)} />}
    {can(rc.RemoteControlRequestKind.AuthorizeController) && <View style={styles.card}>
      <Text style={styles.title}>Authorize another controller</Text>
      <Text>Paste its public controller card and verify its source. This operation gives operator access only. The selected actions replace that controller's prior operator permissions on this node.</Text>
      <TextInput accessibilityLabel="Controller card to authorize" value={input} onChangeText={(value) => { setInput(value); setCard(undefined); setTrusted(false); setPermitted([]); }} multiline autoCapitalize="none" autoCorrect={false} style={styles.input} />
      <Button title="Review controller identity" disabled={disabled || blocked || !input.trim()} onPress={decode} />
      {card !== undefined && <>
        <Text selectable style={styles.fingerprint}>{hex(rc.remoteControlControllerIdentityHash(card.controller).value)}</Text>
        <View style={styles.row}><Switch accessibilityLabel="I verified this controller identity" value={trusted} onValueChange={setTrusted} disabled={disabled || blocked} /><Text>I verified this controller's identity and source.</Text></View>
        <Text>Choose allowed actions:</Text>
        {choices.map((kind) => <View key={kind} style={styles.row}>
          <Switch accessibilityLabel={`Permit ${requestLabel(kind)}`} value={permitted.includes(kind)} disabled={disabled || blocked} onValueChange={(value) => setPermitted((current) => value ? [...current.filter((item) => item !== kind), kind] : current.filter((item) => item !== kind))} />
          <Text>{requestLabel(kind)}</Text>
        </View>)}
        <Button title="Review controller authorization" disabled={disabled || blocked || !trusted || chosen.length === 0} onPress={() => review(new rc.RemoteControlRequest.AuthorizeController({ controller: card.controller, permittedRequests: { kinds: chosen } }))} />
      </>}
    </View>}
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
  </View>;
}
