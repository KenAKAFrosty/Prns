import { runtimeProvider } from "@/native/runtime-provider";
import { Badge, BodyText, Card, Screen, ScreenHeading, Subheading } from "@/ui/primitives";

export function HelpScreen() {
  const { availability } = runtimeProvider;
  const platformName =
    availability.platform === "ios"
      ? "iOS"
      : availability.platform === "android"
        ? "Android"
        : "web";

  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>Help</ScreenHeading>
      <Card>
        <Subheading>What works</Subheading>
        {availability.type === "available" ? (
          <BodyText>
            Create or import an identity, pair and check nearby nodes, save contacts, exchange
            direct messages, inspect this device&apos;s network status, and reset preview data.
          </BodyText>
        ) : (
          <BodyText>
            Browse the responsive app shell, navigate planned pages, choose whether upcoming
            features appear, and view app information.
          </BodyText>
        )}
      </Card>
      <Card>
        <Subheading>Coming later</Subheading>
        {availability.type === "available" ? (
          <BodyText>
            Message details, contact merging, identity and connection management, remote-access
            controls, NomadNet, location, notifications, background delivery, storage details,
            recovery tools, activity history, and support for more platforms.
          </BodyText>
        ) : (
          <BodyText>
            Identity setup, node pairing and status, contacts, direct messaging, app-data reset,
            message details, contact merging, identity and connection management, remote-access
            controls, NomadNet, location, notifications, background delivery, storage details,
            recovery tools, and activity history are not available on {platformName} yet.
          </BodyText>
        )}
      </Card>
      {availability.type === "available" ? (
        <BodyText muted>
          Preview data can be reset and cannot be backed up or recovered yet.
        </BodyText>
      ) : (
        <BodyText muted>
          This preview cannot create, import, or reset app data on {platformName}.
        </BodyText>
      )}
    </Screen>
  );
}
