import { useScaffoldState } from "@/state/scaffold-state-context";
import { NavigationLink } from "@/ui/navigation-link";
import { BodyText, Card, Screen, ScreenHeading, Subheading } from "@/ui/primitives";

export function ExploreScreen() {
  const { state } = useScaffoldState();
  return (
    <Screen>
      <ScreenHeading>Explore</ScreenHeading>
      <BodyText>Explore services available through your Reticulum network.</BodyText>
      {state.showUnavailableFeatures ? (
        <>
          <Card>
            <Subheading>NomadNet</Subheading>
            <BodyText muted>NomadNet browsing is coming later.</BodyText>
            <NavigationLink href="/explore/nomadnet">About NomadNet</NavigationLink>
          </Card>
          <Card>
            <Subheading>Location</Subheading>
            <BodyText muted>Location sharing is coming later.</BodyText>
            <NavigationLink href="/explore/location">About location sharing</NavigationLink>
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
