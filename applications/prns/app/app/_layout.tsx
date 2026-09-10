import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { ScaffoldStateProvider } from "@/state/scaffold-state-context";

export default function RootLayout() {
  return (
    <SafeAreaProvider>
      <ScaffoldStateProvider>
        <StatusBar style="auto" />
        <Stack screenOptions={{ headerShown: false }} />
      </ScaffoldStateProvider>
    </SafeAreaProvider>
  );
}
