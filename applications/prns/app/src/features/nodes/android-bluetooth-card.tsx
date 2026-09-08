import { useState } from "react";
import { Linking } from "react-native";

import { useDevelopmentRuntime } from "@/native/development-runtime-context";
import { Badge, BodyText, Button, Card, Subheading } from "@/ui/primitives";

export function AndroidBluetoothCard() {
  const { androidRuntime } = useDevelopmentRuntime();
  const [pending, setPending] = useState(false);
  const [settingsFailure, setSettingsFailure] = useState<string | null>(null);
  if (androidRuntime === null) return null;
  const { status, failure } = androidRuntime;
  const request = async (operation: () => Promise<void>) => {
    if (pending) return;
    setPending(true);
    setSettingsFailure(null);
    try {
      await operation();
    } catch {
      setSettingsFailure(
        "Settings could not open. Open this app's permissions in Android Settings.",
      );
    } finally {
      setPending(false);
    }
  };
  const permissionNeeded = status !== null && status.bluetoothPermission !== "granted";
  const supported = status?.bluetoothRadio !== "unsupported";
  const ready =
    failure === null &&
    status?.bluetoothPermission === "granted" &&
    status.bluetoothRadio === "on" &&
    status.locationServices !== "off";

  return (
    <Card>
      <Subheading>Bluetooth access</Subheading>
      <Badge tone={ready ? "neutral" : "warning"}>
        {failure !== null
          ? "Status unavailable"
          : status === null
            ? "Checking access"
            : ready
              ? "Ready"
              : "Needs attention"}
      </Badge>
      {status === null ? <BodyText>Checking Bluetooth access…</BodyText> : null}
      {failure === null ? null : (
        <BodyText>Bluetooth access could not be checked. Try refreshing.</BodyText>
      )}
      {status?.bluetoothRadio === "unsupported" ? (
        <BodyText>Bluetooth is not available on this device.</BodyText>
      ) : null}
      {supported && permissionNeeded ? (
        <>
          <BodyText>
            {status?.bluetoothPermission === "blocked"
              ? "Allow Bluetooth access in this app's Android settings to connect to nearby nodes."
              : "Allow Bluetooth access to find and connect to nearby nodes."}
          </BodyText>
          <BodyText muted>
            Your identity, contacts, and other connections do not need Bluetooth access.
          </BodyText>
          <Button
            disabled={pending}
            onPress={() =>
              void request(
                status?.bluetoothPermission === "blocked"
                  ? Linking.openSettings
                  : androidRuntime.requestBluetoothPermissions,
              )
            }
          >
            {status?.bluetoothPermission === "blocked" ? "Open app settings" : "Allow Bluetooth"}
          </Button>
        </>
      ) : null}
      {supported && status?.bluetoothRadio === "off" ? (
        <BodyText>Turn on Bluetooth in Android Settings, then return to prns.</BodyText>
      ) : null}
      {supported && status?.bluetoothRadio === "unknown" ? (
        <BodyText>Bluetooth availability has not been confirmed yet.</BodyText>
      ) : null}
      {supported && status?.locationServices === "off" ? (
        <BodyText>
          Turn on Location in Android Settings to let this version of Android discover nearby
          Bluetooth nodes.
        </BodyText>
      ) : null}
      {ready ? (
        <BodyText>Bluetooth is available. Nearby nodes will appear when discovered.</BodyText>
      ) : null}
      {supported &&
      status?.bluetoothPermission === "granted" &&
      status.backgroundDiscovery === "notGranted" ? (
        <>
          <BodyText>
            Allow background discovery to find nearby nodes while you use other apps. Android may
            ask for location access all the time.
          </BodyText>
          <Button
            disabled={pending}
            tone="secondary"
            onPress={() => void request(androidRuntime.requestBackgroundBluetoothPermission)}
          >
            Allow background discovery
          </Button>
        </>
      ) : null}
      {status?.service === "failed" ? (
        <BodyText>
          This device's node could not keep running. Open its diagnostics for more details.
        </BodyText>
      ) : null}
      {settingsFailure === null ? null : <BodyText>{settingsFailure}</BodyText>}
      <Button
        disabled={pending}
        tone="secondary"
        onPress={() => void request(androidRuntime.refresh)}
      >
        {pending ? "Please wait…" : "Refresh Bluetooth status"}
      </Button>
    </Card>
  );
}
