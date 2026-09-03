import { Badge, BodyText, Card, Screen, ScreenHeading, Subheading } from "@/ui/primitives";

export function HelpScreen() {
  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>Help</ScreenHeading>
      <Card>
        <Subheading>What works</Subheading>
        <BodyText>
          Create or import an identity, pair and check nodes over Bluetooth, save contacts, exchange
          direct messages, inspect this device&apos;s network status, and reset preview data.
        </BodyText>
      </Card>
      <Card>
        <Subheading>Coming later</Subheading>
        <BodyText>
          Message details, contact merging, identity and connection management, remote-access
          controls, NomadNet, location, notifications, background delivery, storage details,
          recovery tools, activity history, and support for more platforms.
        </BodyText>
      </Card>
      <BodyText muted>Preview data can be reset and cannot be backed up or recovered yet.</BodyText>
    </Screen>
  );
}
