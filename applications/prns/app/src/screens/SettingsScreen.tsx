import { Link } from "expo-router";
import { ListRow, PageHeader, Screen, Text } from "../ui/primitives";

export function SettingsScreen() {
  return <Screen><PageHeader title="Settings" />
    <Link href="/setup" asChild><ListRow title="Phone setup" subtitle="Review this phone’s messaging and connection setup" /></Link>
    <Link href="/more/settings/about" asChild><ListRow title="About and diagnostics" subtitle="Version, connection health and support report" /></Link>
    <Text>Message history stays on this phone until you delete it.</Text>
    <Link href="/more/settings/reset" asChild><ListRow title="Reset this phone" subtitle="Review deletion of this app’s local data" /></Link>
  </Screen>;
}
