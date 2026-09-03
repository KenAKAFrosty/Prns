import type { DevelopmentNodeSnapshot } from "@prns-internal/expo";
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
import { formatBytes, formatRequestKind, formatRuntime } from "./format";

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
          <LocalNodeCards snapshot={runtime.snapshot} />
          <Card>
            {runtime.backgroundFailure === null ? null : (
              <BodyText>Snapshot refresh failed: {runtime.backgroundFailure}</BodyText>
            )}
            {refreshFailure === null ? null : (
              <BodyText>Manual refresh failed: {refreshFailure}</BodyText>
            )}
            <Button disabled={refreshing} onPress={() => void refresh()} tone="secondary">
              {refreshing ? "Refreshing…" : "Refresh now"}
            </Button>
            <NavigationLink href="/nodes/local">Open local-node details</NavigationLink>
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

export function LocalNodeScreen() {
  const runtime = useDevelopmentRuntime();
  return (
    <Screen>
      <Badge>Canonical Host snapshot</Badge>
      <ScreenHeading>Local node</ScreenHeading>
      {runtime.snapshot === null ? (
        <Card>
          <Badge tone={runtime.phase === "failed" ? "warning" : "neutral"}>{runtime.phase}</Badge>
          <BodyText>{runtime.lifecycleFailure ?? "Waiting for the native node snapshot."}</BodyText>
        </Card>
      ) : (
        <LocalNodeCards snapshot={runtime.snapshot} />
      )}
      <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
    </Screen>
  );
}

function LocalNodeCards({ snapshot }: { readonly snapshot: DevelopmentNodeSnapshot }) {
  return (
    <>
      <Card>
        <Subheading>Identity and lifecycle</Subheading>
        <Badge tone={snapshot.runtime === "failed" ? "warning" : "neutral"}>
          {formatRuntime(snapshot.runtime)}
        </Badge>
        <PrimaryIdentity identity={snapshot.primaryIdentity} />
        <KeyValue
          label="RemoteControl controller"
          value={
            snapshot.controllerIdentityFingerprint === null
              ? "Not available"
              : formatBytes(snapshot.controllerIdentityFingerprint)
          }
        />
        <KeyValue label="App snapshot revision" value={snapshot.revision.toString()} />
        {snapshot.failure === null ? null : (
          <BodyText>
            {snapshot.failure.stage}: {snapshot.failure.detail}
          </BodyText>
        )}
      </Card>
      <HostCards localHost={snapshot.localHost} />
    </>
  );
}

function PrimaryIdentity({
  identity,
}: {
  readonly identity: DevelopmentNodeSnapshot["primaryIdentity"];
}) {
  switch (identity.type) {
    case "missing":
      return <KeyValue label="Primary identity" value="Missing" />;
    case "present":
      return <KeyValue label="Primary identity" value={formatBytes(identity.identityHash)} />;
    case "unavailable":
      return <KeyValue label="Primary identity" value={`Unavailable — ${identity.detail}`} />;
    case "developmentResetRequired":
      return <KeyValue label="Primary identity" value={`Reset required — ${identity.reason}`} />;
  }
}

function HostCards({ localHost }: { readonly localHost: DevelopmentNodeSnapshot["localHost"] }) {
  if (localHost.type === "stopped") {
    return (
      <Card>
        <Subheading>Host</Subheading>
        <Badge>Stopped</Badge>
        {localHost.lastStartFailure === null ? null : (
          <BodyText>{localHost.lastStartFailure}</BodyText>
        )}
      </Card>
    );
  }
  if (localHost.type === "unavailable" || localHost.type === "developmentResetRequired") {
    return (
      <Card>
        <Subheading>Host</Subheading>
        <Badge tone="warning">
          {localHost.type === "unavailable" ? "Inspection unavailable" : "Reset required"}
        </Badge>
        <BodyText>
          {localHost.type === "unavailable" ? localHost.detail : localHost.reason}
        </BodyText>
      </Card>
    );
  }

  const host = localHost.host;
  return (
    <>
      <Card>
        <Subheading>Host runtime</Subheading>
        <Badge>{host.runtime.running ? "Running" : "Stopped"}</Badge>
        <KeyValue label="Backend" value={host.backend.backend} />
        <KeyValue label="Capabilities" value={host.backend.capabilities.join(", ") || "None"} />
        <KeyValue label="Host revision" value={host.revision.toString()} />
        <KeyValue label="Uptime" value={`${host.runtime.uptimeMillis} ms`} />
        <KeyValue label="Traffic received" value={`${host.runtime.rxBytes.toString()} bytes`} />
        <KeyValue label="Traffic sent" value={`${host.runtime.txBytes.toString()} bytes`} />
        <KeyValue label="Routes" value={host.runtime.routeCount.toString()} />
        <KeyValue label="Active links" value={host.activeLinkCount.toString()} />
      </Card>
      <Card>
        <Subheading>Persistence</Subheading>
        <KeyValue label="Persistent" value={host.persistence.persistent ? "Yes" : "No"} />
        <KeyValue label="Restored" value={host.persistence.restored ? "Yes" : "No"} />
        <KeyValue label="Last flush" value={host.persistence.lastFlushCause ?? "Not observed"} />
        {host.persistence.lastFailureDetail === undefined ? null : (
          <BodyText>{host.persistence.lastFailureDetail}</BodyText>
        )}
      </Card>
      <Subheading>Interfaces</Subheading>
      {host.interfaces.length === 0 ? (
        <Card>
          <Badge tone="warning">No attached interface projected</Badge>
        </Card>
      ) : (
        host.interfaces.map((networkInterface) => (
          <Card key={formatBytes(networkInterface.interfaceId)}>
            <Subheading>{networkInterface.name ?? networkInterface.kind ?? "Interface"}</Subheading>
            <Badge
              tone={
                networkInterface.health === "Failed" || networkInterface.health === "Disabled"
                  ? "warning"
                  : "neutral"
              }
            >
              {networkInterface.health}
            </Badge>
            <KeyValue label="Interface ID" value={formatBytes(networkInterface.interfaceId)} />
            <KeyValue label="Received" value={`${networkInterface.rxBytes.toString()} bytes`} />
            <KeyValue label="Sent" value={`${networkInterface.txBytes.toString()} bytes`} />
            {networkInterface.failureDetail === undefined ? null : (
              <BodyText>{networkInterface.failureDetail}</BodyText>
            )}
          </Card>
        ))
      )}
      <Subheading>Routes</Subheading>
      {host.routes.length === 0 ? (
        <Card>
          <Badge>No live routes</Badge>
        </Card>
      ) : (
        host.routes.map((route) => (
          <Card key={formatBytes(route.destination)}>
            <KeyValue label="Destination" value={formatBytes(route.destination)} />
            <KeyValue label="Hops" value={route.hops.toString()} />
            <KeyValue label="Interface ID" value={formatBytes(route.interfaceId)} />
            <KeyValue
              label="Via identity"
              value={route.viaIdentity === undefined ? "Direct" : formatBytes(route.viaIdentity)}
            />
            <KeyValue label="Expires" value={`${route.expiresAtMillis} ms`} />
          </Card>
        ))
      )}
      <Subheading>Authenticated destination identities</Subheading>
      {host.destinationIdentities.length === 0 ? (
        <Card>
          <Badge>No authenticated observations</Badge>
        </Card>
      ) : (
        host.destinationIdentities.map((association) => (
          <Card key={formatBytes(association.destination)}>
            <KeyValue label="Destination" value={formatBytes(association.destination)} />
            <KeyValue label="Identity" value={formatBytes(association.identity)} />
          </Card>
        ))
      )}
    </>
  );
}
