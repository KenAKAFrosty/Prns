import { useRouter } from "expo-router";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { useScaffoldState } from "@/state/scaffold-state-context";
import { identityFixture } from "@/testkit/fixtures";
import { NavigationLink } from "@/ui/navigation-link";
import {
  Badge,
  BodyText,
  Button,
  Card,
  CardStack,
  KeyValue,
  Screen,
  ScreenHeading,
  Subheading,
} from "@/ui/primitives";
import { radius, space, useAppPalette } from "@/ui/theme";

export function SettingsScreen() {
  const router = useRouter();
  const palette = useAppPalette();
  const { resetDevelopmentData, setOnboardingMode, state, updateScaffoldState } =
    useScaffoldState();
  const selectedIdentity = identityFixture(state.onboardingPreview.selectedIdentityFixtureId);

  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>Settings</ScreenHeading>
      <Card>
        <Subheading>Planned features</Subheading>
        <Pressable
          accessibilityRole="switch"
          accessibilityState={{ checked: state.showUnavailableFeatures }}
          accessibilityLabel="Show unavailable features"
          onPress={() => {
            void updateScaffoldState((current) => ({
              ...current,
              showUnavailableFeatures: !current.showUnavailableFeatures,
            }));
          }}
          style={({ pressed }) => [
            styles.toggle,
            {
              backgroundColor: pressed ? palette.selected : palette.surfaceRaised,
              borderColor: palette.border,
            },
          ]}
        >
          <View style={styles.toggleCopy}>
            <Text style={[styles.toggleLabel, { color: palette.text }]}>
              Show unavailable features
            </Text>
            <Text style={[styles.toggleHint, { color: palette.textMuted }]}>
              Honest placeholders remain routable when hidden.
            </Text>
          </View>
          <Text style={[styles.toggleValue, { color: palette.text }]}>
            {state.showUnavailableFeatures ? "On" : "Off"}
          </Text>
        </Pressable>
      </Card>
      <Card>
        <Subheading>Fixture identity</Subheading>
        {selectedIdentity === undefined ? (
          <BodyText muted>No identity fixture selected.</BodyText>
        ) : (
          <>
            <KeyValue label="Selection" value={selectedIdentity.label} />
            <KeyValue label="Fingerprint" value={selectedIdentity.fingerprint} />
          </>
        )}
        <CardStack>
          <Button
            tone="secondary"
            onPress={() => {
              void updateScaffoldState((current) => ({
                ...current,
                onboardingPreview: {
                  status: "completed",
                  selectedIdentityFixtureId: "identity.generated-preview",
                },
              }));
            }}
          >
            Use generated identity preview
          </Button>
          <Button
            tone="secondary"
            onPress={() => {
              void updateScaffoldState((current) => ({
                ...current,
                onboardingPreview: {
                  status: "completed",
                  selectedIdentityFixtureId: "identity.imported-preview",
                },
              }));
            }}
          >
            Use imported identity preview
          </Button>
        </CardStack>
      </Card>
      <Card>
        <Subheading>Development controls</Subheading>
        <BodyText muted>
          Reopening onboarding does not discard the current preview. Reset removes only the prns
          scaffold key and returns to welcome.
        </BodyText>
        <CardStack>
          <Button
            tone="secondary"
            onPress={() => {
              setOnboardingMode(null);
              router.push("/onboarding/welcome");
            }}
          >
            Reopen onboarding
          </Button>
          <Button tone="secondary" onPress={() => router.replace("/nodes")}>
            Jump to fixture shell
          </Button>
          <Button
            tone="destructive"
            onPress={() => {
              void resetDevelopmentData().then(() => router.replace("/onboarding/welcome"));
            }}
          >
            Reset development data
          </Button>
        </CardStack>
      </Card>
      <CardStack>
        <NavigationLink href="/more/settings/storage">Storage</NavigationLink>
        <NavigationLink href="/more/settings/recovery">Reset and recovery</NavigationLink>
        <NavigationLink href="/more/settings/about">About</NavigationLink>
      </CardStack>
    </Screen>
  );
}

const styles = StyleSheet.create({
  toggle: {
    alignItems: "center",
    borderRadius: radius.sm,
    borderWidth: 1,
    flexDirection: "row",
    gap: space.md,
    justifyContent: "space-between",
    minHeight: 48,
    padding: space.md,
  },
  toggleCopy: { flex: 1, gap: space.xs },
  toggleLabel: { fontSize: 16, fontWeight: "600", lineHeight: 22 },
  toggleHint: { fontSize: 14, lineHeight: 20 },
  toggleValue: { fontSize: 15, fontWeight: "700", lineHeight: 20 },
});
