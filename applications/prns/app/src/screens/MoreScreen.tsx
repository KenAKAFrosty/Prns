import { Link } from "expo-router";
import { View } from "react-native";
import { ListRow, PageHeader, Screen, Text } from "../ui/primitives";

export function MoreScreen() {
  return <Screen><PageHeader title="More" />
    <View style={{ gap: 10 }}><Text accessibilityRole="header">Messaging</Text>
      <Link href="/more/identities" asChild><ListRow title="Messaging identities" subtitle="Names, visibility and messaging preferences" /></Link>
    </View>
    <View style={{ gap: 10 }}><Text accessibilityRole="header">Connectivity</Text>
      <Link href="/more/interfaces" asChild><ListRow title="Connections" subtitle="Bluetooth, local network and TCP servers" /></Link>
      <Link href="/more/network" asChild><ListRow title="Network activity" subtitle="Connections, routes and announcements" /></Link>
    </View>
    <View style={{ gap: 10 }}><Text accessibilityRole="header">App</Text>
      <Link href="/more/settings" asChild><ListRow title="Settings" subtitle="Phone setup, diagnostics and local data" /></Link>
      <Link href="/more/help" asChild><ListRow title="Help" /></Link>
    </View>
  </Screen>;
}
