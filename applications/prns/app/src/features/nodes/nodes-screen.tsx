import type { Href } from "expo-router";
import { useState } from "react";

import { useDevelopmentRuntime } from "@/native/development-runtime-context";
import { NavigationLink } from "@/ui/navigation-link";
import {
  Badge,
  BodyText,
  Button,
  Card,
  CardStack,
  KeyValue,
  Screen,
  ScreenHeading,
  Subheading,
} from "@/ui/primitives";
import { formatBluetooth, formatBytes, formatRequestKind, formatRuntime } from "./format";

export function NodesScreen() {
  const runtime = useDevelopmentRuntime();
  const [refreshFailure, setRefreshFailure] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const refresh = async () => {
    setRefreshing(true);
    setRefreshFailure(null);
    const result = await runtime.refreshSnapshot();
    if (result.type === "operationFailure") {
      setRefreshFailure(result.detail);
    }
    setRefreshing(false);
  };

  return (
    <Screen>
      <Badge>Native development runtime</Badge>
      <ScreenHeading>Nodes</ScreenHeading>
      <BodyText>
        The installation-local controller and its persisted RemoteControl targets come directly from
        the app-owned Rust node.
      </BodyText>

      {runtime.phase === "unavailable" ? (
        <Card>
          <Subheading>Native runtime unavailable</Subheading>
          <Badge tone="warning">Not implemented on {runtime.availability.platform}</Badge>
          <BodyText>
            This platform has no native development-node provider. No target inventory or pairing
            result is being simulated.
          </BodyText>
        </Card>
      ) : null}

      {runtime.phase === "starting" ? (
        <Card>
          <Subheading>Local node</Subheading>
          <Badge>Starting</Badge>
          <BodyText muted>
            Opening private persistence and waiting for the Rust node's bounded readiness signal.
          </BodyText>
        </Card>
      ) : null}

      {runtime.phase === "failed" ? (
        <Card>
          <Subheading>Local node failed to start</Subheading>
          <Badge tone="warning">Startup failed</Badge>
          <BodyText>
            {runtime.lifecycleFailure ?? "The native provider did not return a reason."}
          </BodyText>
        </Card>
      ) : null}

      {runtime.snapshot === null ? null : (
        <>
          <Card>
            <Subheading>Local node</Subheading>
            <Badge tone={runtime.snapshot.runtime === "failed" ? "warning" : "neutral"}>
              {formatRuntime(runtime.snapshot.runtime)}
            </Badge>
            <KeyValue label="Bluetooth Auto" value={formatBluetooth(runtime.snapshot.bluetooth)} />
            <KeyValue
              label="Controller fingerprint"
              value={
                runtime.snapshot.controllerIdentityFingerprint === null
                  ? "Not available"
                  : formatBytes(runtime.snapshot.controllerIdentityFingerprint)
              }
            />
            <KeyValue label="Snapshot revision" value={runtime.snapshot.revision.toString()} />
            {runtime.snapshot.failure === null ? null : (
              <BodyText>
                {runtime.snapshot.failure.stage}: {runtime.snapshot.failure.detail}
              </BodyText>
            )}
            {runtime.backgroundFailure === null ? null : (
              <BodyText>Snapshot refresh failed: {runtime.backgroundFailure}</BodyText>
            )}
            {refreshFailure === null ? null : (
              <BodyText>Manual refresh failed: {refreshFailure}</BodyText>
            )}
            <Button disabled={refreshing} onPress={() => void refresh()} tone="secondary">
              {refreshing ? "Refreshing…" : "Refresh now"}
            </Button>
            <NavigationLink href="/nodes/local">Open local-node placeholder</NavigationLink>
          </Card>

          <Subheading>Persisted managed targets</Subheading>
          {runtime.snapshot.pairedTargets.length === 0 ? (
            <Card>
              <Badge>No persisted targets</Badge>
              <BodyText>
                A target appears here only after controller authorization has been persisted by the
                upstream RemoteControl flow.
              </BodyText>
            </Card>
          ) : (
            runtime.snapshot.pairedTargets.map((target) => {
              const targetId = formatBytes(target.targetIdentityFingerprint);
              const managedHref: Href = {
                pathname: "/nodes/managed/[nodeId]",
                params: { nodeId: targetId },
              };
              return (
                <Card key={targetId}>
                  <Subheading>RemoteControl target</Subheading>
                  <Badge>Persisted authorization</Badge>
                  <KeyValue label="Target fingerprint" value={targetId} />
                  <KeyValue label="Destination" value={formatBytes(target.destination)} />
                  <KeyValue
                    label="Permitted requests"
                    value={
                      target.permittedRequests.length === 0
                        ? "None"
                        : target.permittedRequests.map(formatRequestKind).join(", ")
                    }
                  />
                  <NavigationLink href={managedHref}>Manage this target</NavigationLink>
                </Card>
              );
            })
          )}
        </>
      )}

      <CardStack>
        <NavigationLink href="/nodes/pair">Pair a node</NavigationLink>
        <NavigationLink href="/nodes/local/grants">Controller grants</NavigationLink>
      </CardStack>
    </Screen>
  );
}
