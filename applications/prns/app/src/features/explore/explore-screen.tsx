import { useScaffoldState } from "@/state/scaffold-state-context";
import { Badge, BodyText, Card, Screen, ScreenHeading, Subheading } from "@/ui/primitives";
import { NavigationLink } from "@/ui/navigation-link";

export function ExploreScreen() {
  const { state } = useScaffoldState();
  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>Explore</ScreenHeading>
      <BodyText>
        Planned Reticulum experiences stay discoverable without pretending their services are
        available.
      </BodyText>
      {state.showUnavailableFeatures ? (
        <>
          <Card>
            <Subheading>NomadNet</Subheading>
            <BodyText muted>No request, safe parser, or page cache is compiled.</BodyText>
            <NavigationLink href="/explore/nomadnet">Open placeholder</NavigationLink>
          </Card>
          <Card>
            <Subheading>Location</Subheading>
            <BodyText muted>
              No permission, position, sharing, or map operation is compiled.
            </BodyText>
            <NavigationLink href="/explore/location">Open placeholder</NavigationLink>
          </Card>
        </>
      ) : (
        <Card>
          <BodyText muted>
            Planned unavailable entries are hidden. Restore them from Settings.
          </BodyText>
        </Card>
      )}
    </Screen>
  );
}
