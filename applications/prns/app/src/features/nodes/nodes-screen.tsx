import type { Href } from "expo-router";

import { useScaffoldState } from "@/state/scaffold-state-context";
import { managedNodeFixture } from "@/testkit/fixtures";
import {
  Badge,
  BodyText,
  Card,
  CardStack,
  KeyValue,
  Screen,
  ScreenHeading,
  Subheading,
} from "@/ui/primitives";
import { NavigationLink } from "@/ui/navigation-link";

export function NodesScreen() {
  const { state } = useScaffoldState();
  const fixture = managedNodeFixture(state.selectedManagedNodeFixtureId);
  const managedHref: Href = {
    pathname: "/nodes/managed/[nodeId]",
    params: { nodeId: state.selectedManagedNodeFixtureId },
  };

  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>Nodes</ScreenHeading>
      <BodyText>
        The local application node and remote managed nodes remain separate contexts. This scaffold
        owns neither one yet.
      </BodyText>
      <Card>
        <Subheading>Local node</Subheading>
        <Badge tone="warning">Not yet implemented</Badge>
        <BodyText muted>No aggregate Rust Host is running.</BodyText>
        <NavigationLink href="/nodes/local">Open local-node placeholder</NavigationLink>
      </Card>
      {fixture === undefined ? null : (
        <Card>
          <Subheading>{fixture.label}</Subheading>
          <Badge>Fixture target</Badge>
          <KeyValue label="Fingerprint" value={fixture.fingerprint} />
          <BodyText muted>
            This target cannot pair, connect, or execute RemoteControl commands.
          </BodyText>
          <NavigationLink href={managedHref}>Open managed-node preview</NavigationLink>
        </Card>
      )}
      <CardStack>
        <NavigationLink href="/nodes/pair">Pair a node</NavigationLink>
        <NavigationLink href="/nodes/local/grants">Controller grants</NavigationLink>
      </CardStack>
    </Screen>
  );
}
