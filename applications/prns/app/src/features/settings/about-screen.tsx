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
        <KeyValue label="Platform" value="iOS preview" />
      </Card>
      <BodyText>
        This preview can create or import an identity, pair and check nodes over Bluetooth, save
        contacts, exchange direct messages, and show this device&apos;s network status. New messages
        arrive only while the app is open.
      </BodyText>
    </Screen>
  );
}
