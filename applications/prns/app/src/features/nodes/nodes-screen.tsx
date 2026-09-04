import type { ContactMutationOutcome, DevelopmentNodeSnapshot } from "@prns-internal/expo";
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
      <Badge>Your network</Badge>
      <ScreenHeading>Nodes</ScreenHeading>
      <BodyText>Manage this device and the nodes paired with it.</BodyText>

      {runtime.accessorySetup?.phase === "setupRequired" ? (
        <Card>
          <Subheading>Bluetooth access needed</Subheading>
          <Badge tone="warning">Connection access required</Badge>
          <BodyText>
            Allow a nearby Reticulum Bluetooth node before this device starts searching or
            reconnecting.
          </BodyText>
          <NavigationLink href="/nodes/pair">Set up Bluetooth</NavigationLink>
        </Card>
      ) : null}

      {runtime.accessorySetup?.phase === "failed" || runtime.accessorySetupFailure !== null ? (
        <Card>
          <Subheading>Bluetooth access unavailable</Subheading>
          <Badge tone="warning">Relaunch required</Badge>
          <BodyText>Relaunch prns to check Bluetooth access again.</BodyText>
        </Card>
      ) : null}

      {runtime.phase === "unavailable" ? (
        <Card>
          <Subheading>Nodes unavailable</Subheading>
          <Badge tone="warning">Not supported on {runtime.availability.platform}</Badge>
          <BodyText>Node management is not available on this platform yet.</BodyText>
        </Card>
      ) : null}

      {runtime.phase === "starting" && runtime.accessorySetup?.phase !== "setupRequired" ? (
        <Card>
          <Subheading>Getting ready</Subheading>
          <Badge>Starting</Badge>
          <BodyText muted>Loading your nodes…</BodyText>
        </Card>
      ) : null}

      {runtime.phase === "failed" &&
      runtime.accessorySetup?.phase !== "failed" &&
      runtime.accessorySetupFailure === null ? (
        <Card>
          <Subheading>This device&apos;s node failed to start</Subheading>
          <Badge tone="warning">Startup failed</Badge>
          <BodyText>
            This device&apos;s node could not start. Open its diagnostics for more details.
          </BodyText>
        </Card>
      ) : null}

      {runtime.snapshot === null ? null : (
        <>
          <Card>
            {runtime.backgroundFailure === null ? null : (
              <BodyText>Automatic refresh failed. Try refreshing again.</BodyText>
            )}
            {refreshFailure === null ? null : <BodyText>Refresh failed. Try again.</BodyText>}
            <Button disabled={refreshing} onPress={() => void refresh()} tone="secondary">
              {refreshing ? "Refreshing…" : "Refresh now"}
            </Button>
            <NavigationLink href="/nodes/local">View this device</NavigationLink>
          </Card>

          <Subheading>Paired nodes</Subheading>
          {runtime.snapshot.pairedTargets.length === 0 ? (
            <Card>
              <Badge>No paired nodes</Badge>
              <BodyText>Pair a node to manage it from this device.</BodyText>
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
                  <Subheading>Paired node</Subheading>
                  <Badge>Ready</Badge>
                  <KeyValue label="Node ID" value={targetId} />
                  <KeyValue label="Destination" value={formatBytes(target.destination)} />
                  <KeyValue
                    label="Available actions"
                    value={
                      target.permittedRequests.length === 0
                        ? "None"
                        : target.permittedRequests.map(formatRequestKind).join(", ")
                    }
                  />
                  <NavigationLink href={managedHref}>Manage node</NavigationLink>
                </Card>
              );
            })
          )}
        </>
      )}

      <CardStack>
        <NavigationLink href="/nodes/pair">Pair a node</NavigationLink>
        <NavigationLink href="/nodes/local/grants">Remote access</NavigationLink>
      </CardStack>
    </Screen>
  );
}

export function LocalNodeScreen() {
  const runtime = useDevelopmentRuntime();
  return (
    <Screen>
      <Badge>Node diagnostics</Badge>
      <ScreenHeading>This device</ScreenHeading>
      {runtime.snapshot === null ? (
        <Card>
          <Badge tone={runtime.phase === "failed" ? "warning" : "neutral"}>{runtime.phase}</Badge>
          <BodyText>{runtime.lifecycleFailure ?? "Waiting for node details."}</BodyText>
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
          label="Controller identity"
          value={
            snapshot.controllerIdentityFingerprint === null
              ? "Not available"
              : formatBytes(snapshot.controllerIdentityFingerprint)
          }
        />
        <KeyValue label="Status revision" value={snapshot.revision.toString()} />
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
          <Badge tone="warning">No interfaces attached</Badge>
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
      <Subheading>Verified network identities</Subheading>
      {host.destinationIdentities.length === 0 ? (
        <Card>
          <Badge>No authenticated observations</Badge>
        </Card>
      ) : (
        host.destinationIdentities.map((association) => {
          const associationKey = `${formatBytes(association.destination)}:${formatBytes(association.identity)}`;
          return <ObservedIdentityCard association={association} key={associationKey} />;
        })
      )}
    </>
  );
}

type AuthenticatedAssociation = Extract<
  DevelopmentNodeSnapshot["localHost"],
  { readonly type: "running" }
>["host"]["destinationIdentities"][number];

function ObservedIdentityCard({ association }: { readonly association: AuthenticatedAssociation }) {
  const runtime = useDevelopmentRuntime();
  const [outcome, setOutcome] = useState<ContactMutationOutcome | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const destination = formatBytes(association.destination);
  const contactHref: Href = {
    pathname: "/contacts/[destination]",
    params: { destination },
  };

  const save = async () => {
    setPending(true);
    setFailure(null);
    const result = await runtime.saveObservedDestination(association.destination);
    if (result.type === "operationFailure") {
      setFailure(result.detail);
    } else {
      setOutcome(result.outcome);
    }
    setPending(false);
  };

  const saved =
    outcome?.type === "saved" || outcome?.type === "updated" || outcome?.type === "existing";

  return (
    <Card>
      <KeyValue label="Destination" value={destination} />
      <KeyValue label="Identity" value={formatBytes(association.identity)} />
      <Button disabled={pending} onPress={() => void save()}>
        {pending ? "Saving…" : "Save as contact"}
      </Button>
      {failure === null ? null : <BodyText>{failure}</BodyText>}
      {outcome === null ? null : <BodyText>{observedSaveMessage(outcome)}</BodyText>}
      {saved ? <NavigationLink href={contactHref}>Open saved contact</NavigationLink> : null}
    </Card>
  );
}

function observedSaveMessage(outcome: ContactMutationOutcome): string {
  switch (outcome.type) {
    case "saved":
      return "The verified destination was saved.";
    case "updated":
      return "The verified identity was added to the saved contact.";
    case "existing":
      return "This verified destination was already saved.";
    case "identityConflict":
      return `The saved identity ${formatBytes(outcome.existing)} differs from the observation ${formatBytes(outcome.attempted)}.`;
    case "notObserved":
      return "This destination is no longer visible on the network.";
    case "localNodeStopped":
      return "This device's node stopped before this address could be saved.";
    case "developmentUnavailable":
      return outcome.detail;
    case "developmentResetRequired":
      return `Development reset required: ${outcome.reason}`;
    case "alreadyExists":
      return "This destination is already saved.";
    case "missingIdentity":
      return "The destination does not have an authenticated identity.";
    case "deleted":
      return "The contact was deleted.";
    case "notFound":
      return "The contact was not found.";
  }
}
