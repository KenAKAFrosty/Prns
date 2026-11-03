import { Stack } from "expo-router";
import App from "../src/App";
import { useAppPalette } from "../src/ui/theme";
function Routes() {
  const palette = useAppPalette();
  return <Stack screenOptions={{ headerStyle: { backgroundColor: palette.surface }, headerTintColor: palette.text, headerBackButtonDisplayMode: "minimal", contentStyle: { backgroundColor: palette.background } }}>
    <Stack.Screen name="index" options={{ headerShown: false }} />
    <Stack.Screen name="(shell)" options={{ headerShown: false }} />
    <Stack.Screen name="setup" options={{ title: "Phone setup" }} />
    <Stack.Screen name="setup/import" options={{ title: "Import phone identity" }} />
  </Stack>;
}
export default function RootLayout() { return <App><Routes /></App>; }
