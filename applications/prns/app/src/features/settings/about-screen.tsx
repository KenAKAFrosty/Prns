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
        <KeyValue label="Runtime" value="Presentation scaffold only" />
      </Card>
      <BodyText>
        This build has no Rust Host, RemoteControl provider, identity vault, mailbox, or durable
        application service. The canonical production identifier remains rs.reticulum.prns.
      </BodyText>
    </Screen>
  );
}
