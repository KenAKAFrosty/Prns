import { screenCatalog } from "@/navigation/catalog";
import { useScaffoldState } from "@/state/scaffold-state-context";
import { Badge, BodyText, CardStack, Screen, ScreenHeading } from "@/ui/primitives";
import { NavigationLink } from "@/ui/navigation-link";

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
      <Badge>Development preview</Badge>
      <ScreenHeading>More</ScreenHeading>
      <BodyText>
        Open implemented Settings and Help here. Identities, Interfaces, Notifications, and Activity
        remain clearly labelled placeholders when unavailable features are shown.
      </BodyText>
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
