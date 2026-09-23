import { useState } from "react";
import { Linking } from "react-native";

import { useDevelopmentRuntime } from "@/native/development-runtime-context";
import { Badge, BodyText, Button, Card, Subheading } from "@/ui/primitives";

export function IosBluetoothCard() {
  const runtime = useDevelopmentRuntime();
  const [settingsFailure, setSettingsFailure] = useState(false);
  if (runtime.availability.platform !== "ios") return null;
  const authorization = runtime.bluetoothAuthorization?.authorization;
  const openSettings = async () => {
    setSettingsFailure(false);
    try {
      await Linking.openSettings();
    } catch {
      setSettingsFailure(true);
    }
  };
  if (runtime.bluetoothAuthorizationFailure !== null) {
    return (
      <Card>
        <Subheading>Bluetooth status unavailable</Subheading>
        <BodyText>
          Bluetooth access could not be checked. Your saved data is still available.
        </BodyText>
      </Card>
    );
  }
  if (authorization === "allowedAlways") return null;
  if (authorization === "denied" || authorization === "restricted") {
    return (
      <Card>
        <Subheading>Bluetooth access needed</Subheading>
        <Badge tone="warning">Nearby connections unavailable</Badge>
        <BodyText>
          {authorization === "restricted"
            ? "Bluetooth access is restricted on this device. Check its privacy or device management settings."
            : "Allow Bluetooth in Settings to connect to nearby phones and nodes."}
        </BodyText>
        <BodyText muted>Your identity, contacts, and saved messages are still available.</BodyText>
        {settingsFailure ? (
          <BodyText>Open Settings and find prns to change Bluetooth access.</BodyText>
        ) : null}
        {authorization === "denied" ? (
          <Button onPress={() => void openSettings()}>Open Settings</Button>
        ) : null}
      </Card>
    );
  }
  return (
    <Card>
      <Subheading>Bluetooth access</Subheading>
      <BodyText muted>
        {authorization === "notDetermined"
          ? runtime.canStartNode
            ? "Keep prns open and start this phone to allow Bluetooth. No device pairing is needed."
            : "Allow Bluetooth when prompted to discover nearby phones and nodes. No device pairing is needed."
          : "Checking Bluetooth access…"}
      </BodyText>
      {authorization === "notDetermined" && runtime.canStartNode ? (
        <Button onPress={runtime.startNode}>
          {runtime.phase === "failed" ? "Retry Bluetooth" : "Start this phone"}
        </Button>
      ) : null}
    </Card>
  );
}
