import { Badge, BodyText, Card, Screen, ScreenHeading, Subheading } from "@/ui/primitives";

export function HelpScreen() {
  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>Help</ScreenHeading>
      <Card>
        <Subheading>What works</Subheading>
        <BodyText>
          Responsive navigation, preview onboarding, clearly synthetic fixture selection, local UI
          preferences, and one-key development reset.
        </BodyText>
      </Card>
      <Card>
        <Subheading>What does not work yet</Subheading>
        <BodyText>
          Networking, identities, pairing, messages, contacts, interfaces, NomadNet, location,
          notifications, and recovery are not implemented by this scaffold.
        </BodyText>
      </Card>
      <BodyText muted>
        Development data is disposable. Do not treat fixture names or fingerprints as Reticulum
        material.
      </BodyText>
    </Screen>
  );
}
