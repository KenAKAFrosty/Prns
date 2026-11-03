import { Redirect } from "expo-router";
import { ActivityIndicator } from "react-native";
import { useInstallation } from "../src/runtime/InstallationProvider";
import { Screen } from "../src/ui/primitives";
import { InstallationScreen } from "../src/screens/InstallationScreen";
export default function Index() {
  const { mode, error } = useInstallation();
  if (mode === undefined && error !== undefined) return <InstallationScreen />;
  if (mode === undefined) return <Screen><ActivityIndicator accessibilityLabel="Checking saved installation" /></Screen>;
  return <Redirect href={mode === "create" ? "/setup" : "/conversations"} />;
}
