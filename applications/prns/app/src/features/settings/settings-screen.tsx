import * as Bindings from "@prns-internal/expo";
import { useRouter } from "expo-router";
import { useState } from "react";
import { Alert, Pressable, StyleSheet, Text, View } from "react-native";

import { runtimeProvider } from "@/native/runtime-provider";
import { useScaffoldState } from "@/state/scaffold-state-context";
import { NavigationLink } from "@/ui/navigation-link";
import {
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
      setResetFailure("Reset is not available on this platform yet.");
      return;
    }
    setResetting(true);
    setResetFailure(null);
    try {
      const outcome = await runtimeProvider.runtime.resetDevelopmentData();
      if (outcome.tag === Bindings.DevelopmentNodeStopOutcome_Tags.Failed) {
        setResetFailure("App data could not be reset. Try again.");
        return;
      }
      await resetDevelopmentData();
      router.replace("/onboarding/welcome");
    } catch {
      setResetFailure("App data could not be reset. Try again.");
    } finally {
      setResetting(false);
    }
  };

  const confirmReset = () => {
    Alert.alert(
      "Reset app data?",
      "This permanently removes your primary identity, paired-node access, saved contacts, messages, and app preferences from this device.",
      [
        { text: "Cancel", style: "cancel" },
        { text: "Reset", style: "destructive", onPress: () => void reset() },
      ],
    );
  };

  return (
    <Screen>
      <ScreenHeading>Settings</ScreenHeading>
      <Card>
        <Subheading>Upcoming features</Subheading>
        <Pressable
          accessibilityRole="switch"
          accessibilityState={{ checked: state.showUnavailableFeatures }}
          accessibilityLabel="Show upcoming features"
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
              Show upcoming features
            </Text>
            <Text style={[styles.toggleHint, { color: palette.textMuted }]}>
              Include pages for features that are still being built.
            </Text>
          </View>
          <Text style={[styles.toggleValue, { color: palette.text }]}>
            {state.showUnavailableFeatures ? "On" : "Off"}
          </Text>
        </Pressable>
      </Card>
      <Card>
        <Subheading>App data</Subheading>
        <BodyText muted>
          Reset removes this app&apos;s identity, paired nodes, contacts, messages, and preferences
          from this device.
        </BodyText>
        <CardStack>
          <Button
            disabled={resetting || runtimeProvider.availability.type !== "available"}
            tone="destructive"
            onPress={confirmReset}
          >
            {resetting ? "Resetting…" : "Reset app data"}
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
