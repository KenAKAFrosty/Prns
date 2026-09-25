import { DevelopmentNodeRuntime } from "@prns-internal/expo";
import { useState } from "react";

import { AndroidBluetoothCard } from "@/features/nodes/android-bluetooth-card";
import { IosBluetoothCard } from "@/features/nodes/ios-bluetooth-card";
import { formatBytes, formatRuntime } from "@/features/nodes/format";
import { useDevelopmentRuntime } from "@/native/development-runtime-context";
import { NavigationLink } from "@/ui/navigation-link";
import {
  ActionRow,
  Badge,
  BodyText,
  Button,
  Card,
  CardHeader,
  CardSection,
  KeyValue,
  Screen,
  ScreenHeading,
} from "@/ui/primitives";
import { bluetoothStatus } from "./bluetooth-status";

export function ConnectionsScreen() {
  const runtime = useDevelopmentRuntime();
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const bluetooth = runtime.snapshot?.bluetooth;
  const status = bluetoothStatus(runtime);
  const disabled =
    pending ||
    runtime.phase === "starting" ||
    runtime.stoppingNode ||
    bluetooth?.desiredEnabled === undefined ||
    bluetooth.state.tag === "Disabling";

  const changeBluetooth = async () => {
    if (disabled || bluetooth?.desiredEnabled === undefined) return;
    setPending(true);
    setFailure(null);
    try {
      const result = await runtime.setBluetoothEnabled(!bluetooth.desiredEnabled);
      if (result.type === "operationFailure") setFailure(result.detail);
      else if (result.outcome.tag === "Busy")
        setFailure("This phone is changing connections. Wait a moment and try again.");
      else if (result.outcome.tag === "Unavailable")
        setFailure(
          "The Bluetooth setting could not be confirmed. Refresh to check it, then try again.",
        );
    } finally {
      setPending(false);
    }
  };
  const refresh = async () => {
    if (pending) return;
    setPending(true);
    setFailure(null);
    try {
      await runtime.androidRuntime?.refresh();
      const result = await runtime.refreshSnapshot();
      if (result.type === "operationFailure") setFailure(result.detail);
    } finally {
      setPending(false);
    }
  };

  return (
    <Screen>
      <ScreenHeading>Connections</ScreenHeading>
      <BodyText>Connect this phone to nearby devices using Bluetooth.</BodyText>
      <Card>
        <CardHeader title="This phone">
          <Badge>
            {runtime.snapshot === null ? "Getting ready" : formatRuntime(runtime.snapshot.runtime)}
          </Badge>
        </CardHeader>
        {runtime.canStartNode ? <Button onPress={runtime.startNode}>Start node</Button> : null}
        <NavigationLink href="/nodes/local">Node diagnostics</NavigationLink>
      </Card>
      <Card>
        <CardHeader title="Bluetooth connections">
          <Badge tone={status.warning ? "warning" : "neutral"}>{status.label}</Badge>
        </CardHeader>
        <BodyText>{status.description}</BodyText>
        {runtime.availability.type === "available" ? (
          <>
            <BodyText muted>
              Turning this off disconnects nearby devices. Your identity, contacts, and messages
              stay saved.
              {runtime.snapshot?.runtime === DevelopmentNodeRuntime.Running
                ? " Your node keeps running."
                : ""}
            </BodyText>
            <ActionRow>
              <Button disabled={disabled} onPress={() => void changeBluetooth()} tone="secondary">
                {pending
                  ? "Please wait…"
                  : bluetooth?.desiredEnabled === false
                    ? "Enable Bluetooth connections"
                    : "Disable Bluetooth connections"}
              </Button>
              <Button disabled={pending} onPress={() => void refresh()} tone="secondary">
                Refresh connections
              </Button>
            </ActionRow>
          </>
        ) : null}
        {failure === null ? null : <BodyText>{failure}</BodyText>}
        {status.showConnectedPeers
          ? bluetooth?.peers
              .filter((peer) => peer.connected)
              .map((peer) => (
                <CardSection
                  key={formatBytes(peer.interfaceId)}
                  title={peer.name ?? "Nearby device"}
                >
                  <Badge>Connected</Badge>
                  <KeyValue label="Connection ID" value={formatBytes(peer.interfaceId)} />
                  {peer.details === undefined ? null : <BodyText>{peer.details}</BodyText>}
                  {peer.rssiDbm === undefined ? null : (
                    <KeyValue label="Signal" value={`${peer.rssiDbm} dBm`} />
                  )}
                  <KeyValue label="Received" value={`${peer.rxBytes.toString()} bytes`} />
                  <KeyValue label="Sent" value={`${peer.txBytes.toString()} bytes`} />
                </CardSection>
              ))
          : null}
      </Card>
      <AndroidBluetoothCard />
      <IosBluetoothCard />
      <NavigationLink href="/nodes" direction="back">
        Back to Nodes
      </NavigationLink>
    </Screen>
  );
}
