import type { PropsWithChildren } from "react";
import { SafeAreaProvider } from "react-native-safe-area-context";
import { StatusBar } from "expo-status-bar";
import { View } from "react-native";
import { InstallationProvider, useInstallation } from "./runtime/InstallationProvider";
import { InstallationScreen } from "./screens/InstallationScreen";
import { ResetScreen } from "./screens/ResetScreen";

function ResetGate({ children }: PropsWithChildren) {
  const { resetPending, presentationEpoch } = useInstallation();
  // Keep Expo's navigator mounted while a confirmed reset completes. All
  // presentation writes are separately fenced by the reset storage barrier.
  return <View style={{ flex: 1 }}>
    <View key={presentationEpoch} style={{ flex: 1, display: resetPending ? "none" : "flex" }} pointerEvents={resetPending ? "none" : "auto"}>{children}</View>
    {resetPending && <ResetScreen />}
  </View>;
}

export default function App({ children }: PropsWithChildren) {
  return <SafeAreaProvider><StatusBar style="auto" /><InstallationProvider>
    <ResetGate>{children ?? <InstallationScreen />}</ResetGate>
  </InstallationProvider></SafeAreaProvider>;
}
