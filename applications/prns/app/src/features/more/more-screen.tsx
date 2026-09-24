import { screenCatalog } from "@/navigation/catalog";
import { useScaffoldState } from "@/state/scaffold-state-context";
import { NavigationLink } from "@/ui/navigation-link";
import { BodyText, CardStack, Screen, ScreenHeading } from "@/ui/primitives";

const moreEntries = [
  "identities.index",
  "interfaces.index",
  "notifications.settings",
  "activity.index",
  "settings.index",
  "help.index",
] as const;

export function MoreScreen() {
  const { state } = useScaffoldState();
  const entries = screenCatalog.filter(
    (entry) =>
      moreEntries.some((id) => id === entry.id) &&
      (state.showUnavailableFeatures || entry.availability === "implementedScaffold"),
  );
  return (
    <Screen>
      <ScreenHeading>More</ScreenHeading>
      <BodyText>App settings, connections, and help.</BodyText>
      <CardStack>
        {entries.map((entry) => (
          <NavigationLink key={entry.id} href={entry.path}>
            {entry.label}
          </NavigationLink>
        ))}
      </CardStack>
    </Screen>
  );
}
