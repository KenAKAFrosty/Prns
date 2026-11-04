import { Button, Text, TextInput } from "../ui/primitives";
import { useState } from "react";
import { View } from "react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { useStyles } from "../ui";
import { Facts } from "./presentation";

/** Revision is always read from the node; the UI never invents a transaction. */
export function Wifi({ supported, status, disabled, blocked, review, refresh }: {
  supported: rc.RemoteControlRequestSet; status: rc.RemoteControlWifiTransactionStatus | undefined; disabled: boolean; blocked: boolean;
  review(request: rc.RemoteControlRequest): void; refresh(): void;
}) {
  const styles = useStyles();
  const [ssid, setSsid] = useState("");
  const [password, setPassword] = useState("");
  const can = (kind: rc.RemoteControlRequestKind) => supported.kinds.includes(kind);
  if (!can(rc.RemoteControlRequestKind.InspectWifiTransaction) && !can(rc.RemoteControlRequestKind.StageWifiCredentials)) return null;
  return <View style={styles.card}>
    <Text style={styles.title}>Wi-Fi setup</Text>
    <Text>Stage new credentials, activate the returned revision, then confirm after checking the connection. Changing Wi-Fi can disconnect this session. No step is automatically retried.</Text>
    {can(rc.RemoteControlRequestKind.InspectWifiTransaction) && <Button title="Read Wi-Fi setup status" onPress={refresh} disabled={disabled} />}
    {status !== undefined && <Facts value={status} />}
    {can(rc.RemoteControlRequestKind.StageWifiCredentials) && <>
      <TextInput accessibilityLabel="Wi-Fi network name" value={ssid} onChangeText={setSsid} autoCapitalize="none" autoCorrect={false} style={styles.input} />
      <TextInput accessibilityLabel="Wi-Fi password" value={password} onChangeText={setPassword} secureTextEntry autoCapitalize="none" autoCorrect={false} style={styles.input} />
      <Button title="Review staged Wi-Fi credentials" disabled={disabled || blocked || ssid.length === 0} onPress={() => review(new rc.RemoteControlRequest.StageWifiCredentials({ station: { ssid, password } }))} />
    </>}
    {status?.tag === rc.RemoteControlWifiTransactionStatus_Tags.Staged && can(rc.RemoteControlRequestKind.ActivateWifiCredentials) && <Button title="Review activating staged Wi-Fi revision" disabled={disabled || blocked} onPress={() => review(new rc.RemoteControlRequest.ActivateWifiCredentials({ revision: status.inner.revision }))} />}
    {status?.tag === rc.RemoteControlWifiTransactionStatus_Tags.AwaitingConfirmation && can(rc.RemoteControlRequestKind.ConfirmWifiCredentials) && <Button title="Review confirming working Wi-Fi revision" disabled={disabled || blocked} onPress={() => review(new rc.RemoteControlRequest.ConfirmWifiCredentials({ revision: status.inner.revision }))} />}
    {(status?.tag === rc.RemoteControlWifiTransactionStatus_Tags.Staged || status?.tag === rc.RemoteControlWifiTransactionStatus_Tags.AwaitingConfirmation) && can(rc.RemoteControlRequestKind.CancelWifiCredentials) && <Button title="Review cancelling this Wi-Fi revision" disabled={disabled || blocked} onPress={() => review(new rc.RemoteControlRequest.CancelWifiCredentials({ revision: status.inner.revision }))} />}
  </View>;
}
