import Constants from "expo-constants";

import { runtimeProvider } from "@/native/runtime-provider";
import { Badge, BodyText, Card, KeyValue, Screen, ScreenHeading } from "@/ui/primitives";

export function AboutScreen() {
  const { availability } = runtimeProvider;
  const platformName =
    availability.platform === "ios" ? "iOS" : availability.platform === "web" ? "Web" : "Android";
  const platformReference = availability.platform === "ios" ? "iOS" : availability.platform;
  const nativeIdentifier =
    availability.platform === "ios"
      ? (Constants.expoConfig?.ios?.bundleIdentifier ?? "rs.reticulum.prns.dev")
      : availability.platform === "android"
        ? (Constants.expoConfig?.android?.package ?? "rs.reticulum.prns.dev")
        : null;

  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>About prns</ScreenHeading>
      <Card>
        <KeyValue label="Application" value={Constants.expoConfig?.name ?? "prns dev"} />
        <KeyValue label="Application version" value={Constants.expoConfig?.version ?? "0.0.0"} />
        {nativeIdentifier === null ? null : (
          <KeyValue label="Native identifier" value={nativeIdentifier} />
        )}
        <KeyValue label="Platform" value={`${platformName} preview`} />
      </Card>
      {availability.type === "available" ? (
        <BodyText>
          This preview can create or import an identity, pair and check nodes over Bluetooth, save
          contacts, exchange direct messages, and show this device&apos;s network status. New
          messages arrive only while the app is open.
        </BodyText>
      ) : (
        <BodyText>
          This {platformReference} preview lets you explore the app&apos;s layout, navigation,
          settings, and planned pages. Identity setup, node management, contacts, and direct
          messaging are not available on {platformReference} yet.
        </BodyText>
      )}
    </Screen>
  );
}
