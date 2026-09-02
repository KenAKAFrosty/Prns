// route-kind: bootstrap
import { useRouter } from "expo-router";
import { useEffect } from "react";
import { ActivityIndicator, StyleSheet, View } from "react-native";

import { useScaffoldState } from "@/state/scaffold-state-context";
import { useAppPalette } from "@/ui/theme";

export default function BootstrapRoute() {
  const router = useRouter();
  const palette = useAppPalette();
  const { state, status } = useScaffoldState();

  useEffect(() => {
    if (status !== "ready") {
      return;
    }
    router.replace(
      state.onboardingPreview.status === "notStarted" ? "/onboarding/welcome" : "/inbox",
    );
  }, [router, state.onboardingPreview.status, status]);

  return (
    <View style={[styles.loading, { backgroundColor: palette.background }]}>
      <ActivityIndicator
        accessibilityLabel="Loading development preview state"
        color={palette.accent}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  loading: { alignItems: "center", flex: 1, justifyContent: "center" },
});
