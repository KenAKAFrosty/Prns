import { useRouter } from "expo-router";
import { useState } from "react";
import { Alert, Pressable, StyleSheet, Text, View } from "react-native";

import { runtimeProvider } from "@/native/runtime-provider";
import { useScaffoldState } from "@/state/scaffold-state-context";
import { NavigationLink } from "@/ui/navigation-link";
import {
  Badge,
  BodyText,
  Button,
  Card,
  CardStack,
  Screen,
  ScreenHeading,
  Subheading,
} from "@/ui/primitives";
import { radius, space, useAppPalette } from "@/ui/theme";

export function SettingsScreen() {
  const router = useRouter();
  const palette = useAppPalette();
  const { resetDevelopmentData, state, updateScaffoldState } = useScaffoldState();
  const [resetting, setResetting] = useState(false);
  const [resetFailure, setResetFailure] = useState<string | null>(null);

  const reset = async () => {
    if (!("runtime" in runtimeProvider)) {
      setResetFailure("Native development data reset is available only in the iOS build.");
      return;
    }
    setResetting(true);
    setResetFailure(null);
    try {
      const outcome = await runtimeProvider.runtime.resetDevelopmentData();
      if (outcome.type === "failed") {
        setResetFailure(`${outcome.stage}: ${outcome.detail}`);
        return;
      }
      await resetDevelopmentData();
      router.replace("/onboarding/welcome");
    } catch (error) {
      setResetFailure(error instanceof Error ? error.message : String(error));
    } finally {
      setResetting(false);
    }
  };

  const confirmReset = () => {
    Alert.alert(
      "Reset development data?",
      "This stops the local node and permanently removes the primary identity, Bluetooth identity, RemoteControl identities and authorization state from this development install.",
      [
        { text: "Cancel", style: "cancel" },
        { text: "Reset", style: "destructive", onPress: () => void reset() },
      ],
    );
  };

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
        <Subheading>Development controls</Subheading>
        <BodyText muted>
          Reset stops the native node before deleting only this app&apos;s disposable
          prns/development root, then returns to onboarding.
        </BodyText>
        <CardStack>
          <Button
            disabled={resetting || runtimeProvider.availability.type !== "available"}
            tone="destructive"
            onPress={confirmReset}
          >
            {resetting ? "Resetting…" : "Reset development data"}
          </Button>
        </CardStack>
        {resetFailure === null ? null : <BodyText>{resetFailure}</BodyText>}
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
