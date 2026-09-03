import Constants from "expo-constants";

import { Badge, BodyText, Card, KeyValue, Screen, ScreenHeading } from "@/ui/primitives";

export function AboutScreen() {
  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>About prns</ScreenHeading>
      <Card>
        <KeyValue label="Application" value={Constants.expoConfig?.name ?? "prns dev"} />
        <KeyValue label="Application version" value={Constants.expoConfig?.version ?? "0.0.0"} />
        <KeyValue label="Native identifier" value="rs.reticulum.prns.dev" />
        <KeyValue label="Runtime" value="Rust-owned iOS development node" />
      </Card>
      <BodyText>
        This build includes the Rust-owned primary identity vault, local Host inspection, Bluetooth
        Auto, upstream RemoteControl pairing and Describe, a native saved-contact directory, and a
        durable direct-LXMF Inbox and Outbox. Delivery is foreground-only, and the native
        capabilities are iOS-only. The canonical production identifier remains rs.reticulum.prns.
      </BodyText>
    </Screen>
  );
}
