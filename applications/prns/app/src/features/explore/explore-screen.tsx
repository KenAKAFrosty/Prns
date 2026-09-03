import { useScaffoldState } from "@/state/scaffold-state-context";
import { Badge, BodyText, Card, Screen, ScreenHeading, Subheading } from "@/ui/primitives";
import { NavigationLink } from "@/ui/navigation-link";

export function ExploreScreen() {
  const { state } = useScaffoldState();
  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>Explore</ScreenHeading>
      <BodyText>Explore services available through your Reticulum network.</BodyText>
      {state.showUnavailableFeatures ? (
        <>
          <Card>
            <Subheading>NomadNet</Subheading>
            <BodyText muted>NomadNet browsing is coming later.</BodyText>
            <NavigationLink href="/explore/nomadnet">View details</NavigationLink>
          </Card>
          <Card>
            <Subheading>Location</Subheading>
            <BodyText muted>Location sharing is coming later.</BodyText>
            <NavigationLink href="/explore/location">View details</NavigationLink>
          </Card>
        </>
      ) : (
        <Card>
          <BodyText muted>Upcoming features are hidden. You can show them in Settings.</BodyText>
        </Card>
      )}
    </Screen>
  );
}
