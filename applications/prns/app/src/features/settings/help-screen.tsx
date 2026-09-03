import { Badge, BodyText, Card, Screen, ScreenHeading, Subheading } from "@/ui/primitives";

export function HelpScreen() {
  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>Help</ScreenHeading>
      <Card>
        <Subheading>What works</Subheading>
        <BodyText>
          Responsive navigation, native identity onboarding, local-node Host inspection, Bluetooth
          Auto, upstream RemoteControl pairing and Describe, native saved contacts, a durable
          direct-LXMF Inbox and Outbox, local UI preferences, and confirmed development reset.
        </BodyText>
      </Card>
      <Card>
        <Subheading>What does not work yet</Subheading>
        <BodyText>
          The dedicated message inspector, contact merging, separate identity and interface views,
          interface mutation, controller-grant inspection, NomadNet, location, notifications and
          background delivery, storage and retained-recovery inspection, activity diagnostics, and
          non-iOS native runtimes remain placeholders.
        </BodyText>
      </Card>
      <BodyText muted>
        Development data is disposable. Displayed identity hashes and network observations are live
        Reticulum values, but this build provides no backup, recovery, or continuity promise.
      </BodyText>
    </Screen>
  );
}
