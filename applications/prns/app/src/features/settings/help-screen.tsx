import { Badge, BodyText, Card, Screen, ScreenHeading, Subheading } from "@/ui/primitives";

export function HelpScreen() {
  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>Help</ScreenHeading>
      <Card>
        <Subheading>What works</Subheading>
        <BodyText>
          Responsive navigation, native identity onboarding, local-node Host inspection, local UI
          preferences, and confirmed development reset.
        </BodyText>
      </Card>
      <Card>
        <Subheading>What does not work yet</Subheading>
        <BodyText>
          Messages, contacts, interface mutation, NomadNet, location, notifications, recovery, and
          non-iOS native runtimes are not implemented by this development build.
        </BodyText>
      </Card>
      <BodyText muted>
        Development data is disposable. Displayed identity hashes and network observations are live
        Reticulum values, but this build provides no backup, recovery, or continuity promise.
      </BodyText>
    </Screen>
  );
}
