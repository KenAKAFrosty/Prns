// route-kind: bootstrap
import { useRouter } from "expo-router";
import { useEffect } from "react";
import { ActivityIndicator, StyleSheet, View } from "react-native";

import { runtimeProvider } from "@/native/runtime-provider";
import { useScaffoldState } from "@/state/scaffold-state-context";
import { useAppPalette } from "@/ui/theme";

export default function BootstrapRoute() {
  const router = useRouter();
  const palette = useAppPalette();
  const { status } = useScaffoldState();

  useEffect(() => {
    if (status !== "ready") {
      return;
    }
    let active = true;
    if (!("runtime" in runtimeProvider)) {
      router.replace("/onboarding/welcome");
      return;
    }
    void runtimeProvider.runtime
      .inspectDevelopmentIdentity()
      .then((identity) => {
        if (!active) {
          return;
        }
        switch (identity.type) {
          case "present":
            router.replace("/nodes");
            break;
          case "missing":
            router.replace("/onboarding/welcome");
            break;
          case "unavailable":
          case "developmentResetRequired":
            router.replace("/recovery");
            break;
        }
      })
      .catch(() => {
        if (active) {
          router.replace("/recovery");
        }
      });
    return () => {
      active = false;
    };
  }, [router, status]);

  return (
    <View style={[styles.loading, { backgroundColor: palette.background }]}>
      <ActivityIndicator
        accessibilityLabel="Inspecting development identity"
        color={palette.accent}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  loading: { alignItems: "center", flex: 1, justifyContent: "center" },
});
