import { useLocalSearchParams } from "expo-router";

import { screenById, type RawRouteParams, routeParamsAreValid } from "@/navigation/catalog";
import { useScaffoldState } from "@/state/scaffold-state-context";
import { managedNodeFixture } from "@/testkit/fixtures";
import {
  Badge,
  BodyText,
  Card,
  KeyValue,
  Screen,
  ScreenHeading,
  Subheading,
} from "@/ui/primitives";
import { NavigationLink } from "@/ui/navigation-link";
import { NotFoundScreen } from "../placeholder-screen";

export function ManagedNodeScreen() {
  const entry = screenById("nodes.managed");
  const params: RawRouteParams = useLocalSearchParams();
  const nodeId = params.nodeId;
  const { state } = useScaffoldState();

  if (
    !routeParamsAreValid(entry, params) ||
    typeof nodeId !== "string" ||
    nodeId !== state.selectedManagedNodeFixtureId
  ) {
    return <NotFoundScreen backPath="/nodes" />;
  }
  const fixture = managedNodeFixture(state.selectedManagedNodeFixtureId);
  if (fixture === undefined) {
    return <NotFoundScreen backPath="/nodes" />;
  }

  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>{fixture.label}</ScreenHeading>
      <Card>
        <Subheading>Managed target</Subheading>
        <KeyValue label="Name" value={fixture.label} />
        <KeyValue label="Fingerprint" value={fixture.fingerprint} />
        <KeyValue label="Connection" value="Fixture only — no protocol connection" />
      </Card>
      <Badge tone="warning">RemoteControl unavailable</Badge>
      <BodyText>
        The target context stays visible, but no synthetic Describe result or successful command is
        shown. Pairing and management arrive with the native iPhone-to-E290 slice.
      </BodyText>
      <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
    </Screen>
  );
}
