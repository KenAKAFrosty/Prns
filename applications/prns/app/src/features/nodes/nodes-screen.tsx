import * as Bindings from "@prns-internal/expo";
import type { ContactMutationOutcome, DevelopmentNodeSnapshot } from "@prns-internal/expo";
import type { Href } from "expo-router";
import { useState } from "react";

import { useDevelopmentRuntime } from "@/native/development-runtime-context";
import { NavigationLink } from "@/ui/navigation-link";
import {
  ActionRow,
  Badge,
  BodyText,
  Button,
  Card,
  CardHeader,
  CardSection,
  KeyValue,
  Screen,
  ScreenHeading,
  Subheading,
} from "@/ui/primitives";
import { formatBytes, formatRuntime } from "./format";
import { AndroidBluetoothCard } from "./android-bluetooth-card";
import { AndroidNodeControls } from "./android-node-controls";
import { summarizePairingAccess } from "./pairing-access-summary";

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
      <ScreenHeading>Nodes</ScreenHeading>
      <BodyText>Manage your paired nodes and this device.</BodyText>

      <Card>
        <CardHeader title="Paired nodes">
          {runtime.snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Running &&
          runtime.snapshot.pairedTargets.length > 0 ? (
            <Badge>Paired</Badge>
          ) : null}
        </CardHeader>
        {runtime.snapshot === null ? null : runtime.snapshot.runtime !==
          Bindings.DevelopmentNodeRuntime.Running ? (
          <>
            <Badge>Paired nodes unavailable</Badge>
            <BodyText>
              Your paired nodes will appear when this device&apos;s node is running.
            </BodyText>
          </>
        ) : runtime.snapshot.pairedTargets.length === 0 ? (
          <>
            <Badge>No paired nodes</Badge>
            <BodyText>Pair a node to manage it from this device.</BodyText>
          </>
        ) : (
          runtime.snapshot.pairedTargets.map((target) => {
            const targetId = formatBytes(target.targetIdentityFingerprint);
            const managedHref: Href = {
              pathname: "/nodes/managed/[nodeId]",
              params: { nodeId: targetId },
            };
            return (
              <CardSection key={targetId}>
                <KeyValue label="Node ID" value={targetId} />
                <KeyValue
                  label="Available controls"
                  value={
                    target.permittedRequests.length === 0
                      ? "None"
                      : summarizePairingAccess(target.permittedRequests)
                  }
                />
                <ActionRow>
                  <NavigationLink href={managedHref}>Manage node</NavigationLink>
                </ActionRow>
              </CardSection>
            );
          })
        )}
        <CardSection>
          <ActionRow>
            <NavigationLink href="/nodes/pair">Pair a node</NavigationLink>
          </ActionRow>
        </CardSection>
      </Card>

      <NodeRecoveryCard showDiagnosticsLink />

      <Card>
        <CardHeader title="This device">
          {runtime.snapshot === null ? null : (
            <Badge
              tone={
                runtime.snapshot.runtime === Bindings.DevelopmentNodeRuntime.Failed
                  ? "warning"
                  : "neutral"
              }
            >
              {formatRuntime(runtime.snapshot.runtime)}
            </Badge>
          )}
        </CardHeader>
        {runtime.snapshot === null ? null : (
          <>
            {runtime.backgroundFailure === null ? null : (
              <BodyText>Automatic refresh failed. Try refreshing again.</BodyText>
            )}
            {refreshFailure === null ? null : <BodyText>Refresh failed. Try again.</BodyText>}
            <ActionRow>
              <Button disabled={refreshing} onPress={() => void refresh()} tone="secondary">
                {refreshing ? "Refreshing…" : "Refresh now"}
              </Button>
              <NavigationLink href="/nodes/local">View this device</NavigationLink>
            </ActionRow>
          </>
        )}
        <CardSection>
          <NavigationLink href="/nodes/local/grants">Remote access</NavigationLink>
        </CardSection>
      </Card>

      <AndroidBluetoothCard />

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

      <AndroidNodeControls />
    </Screen>
  );
}

export function LocalNodeScreen() {
  const runtime = useDevelopmentRuntime();
  return (
    <Screen>
      <Badge>Node diagnostics</Badge>
      <ScreenHeading>This device</ScreenHeading>
      <NodeRecoveryCard />
      <AndroidNodeControls />
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

function NodeRecoveryCard({
  showDiagnosticsLink = false,
}: {
  readonly showDiagnosticsLink?: boolean;
}) {
  const runtime = useDevelopmentRuntime();
  const stopped = runtime.snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Stopped;
  const failed =
    (runtime.phase === "failed" ||
      runtime.snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Failed) &&
    runtime.accessorySetup?.phase !== "failed" &&
    runtime.accessorySetupFailure === null;
  if (!stopped && !failed) return null;
  return (
    <Card>
      <Subheading>
        {stopped ? "This device's node is stopped" : "This device's node failed to start"}
      </Subheading>
      <Badge tone={stopped ? "neutral" : "warning"}>{stopped ? "Stopped" : "Startup failed"}</Badge>
      <BodyText>
        {stopped
          ? "Start it to reconnect and manage your paired nodes."
          : "This device's node could not start. Open its diagnostics for more details."}
      </BodyText>
      {runtime.canStartNode ? <Button onPress={runtime.startNode}>Start node</Button> : null}
      {showDiagnosticsLink ? (
        <NavigationLink href="/nodes/local">View diagnostics</NavigationLink>
      ) : null}
    </Card>
  );
}

function LocalNodeCards({ snapshot }: { readonly snapshot: DevelopmentNodeSnapshot }) {
  return (
    <>
      <Card>
        <Subheading>Identity and lifecycle</Subheading>
        <Badge
          tone={snapshot.runtime === Bindings.DevelopmentNodeRuntime.Failed ? "warning" : "neutral"}
        >
          {formatRuntime(snapshot.runtime)}
        </Badge>
        <PrimaryIdentity identity={snapshot.primaryIdentity} />
        <KeyValue
          label="Controller identity"
          value={
            snapshot.controllerIdentityFingerprint === undefined
              ? "Not available"
              : formatBytes(snapshot.controllerIdentityFingerprint)
          }
        />
        <KeyValue label="Status revision" value={snapshot.revision.toString()} />
        {snapshot.failure === undefined ? null : (
          <BodyText>
            {Bindings.DevelopmentNodeFailureStage[snapshot.failure.stage]}:{" "}
            {snapshot.failure.detail}
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
  switch (identity.tag) {
    case Bindings.PrimaryIdentityState_Tags.Missing:
      return <KeyValue label="Primary identity" value="Missing" />;
    case Bindings.PrimaryIdentityState_Tags.Present:
      return <KeyValue label="Primary identity" value={formatBytes(identity.inner.identityHash)} />;
    case Bindings.PrimaryIdentityState_Tags.Unavailable:
      return <KeyValue label="Primary identity" value={`Unavailable — ${identity.inner.detail}`} />;
    case Bindings.PrimaryIdentityState_Tags.DevelopmentResetRequired:
      return (
        <KeyValue label="Primary identity" value={`Reset required — ${identity.inner.reason}`} />
      );
  }
}

function HostCards({ localHost }: { readonly localHost: DevelopmentNodeSnapshot["localHost"] }) {
  if (localHost.tag === Bindings.LocalHostState_Tags.Stopped) {
    return (
      <Card>
        <Subheading>Host</Subheading>
        <Badge>Stopped</Badge>
        {localHost.inner.lastStartFailure === undefined ? null : (
          <BodyText>{localHost.inner.lastStartFailure}</BodyText>
        )}
      </Card>
    );
  }
  if (
    localHost.tag === Bindings.LocalHostState_Tags.Unavailable ||
    localHost.tag === Bindings.LocalHostState_Tags.DevelopmentResetRequired
  ) {
    return (
      <Card>
        <Subheading>Host</Subheading>
        <Badge tone="warning">
          {localHost.tag === Bindings.LocalHostState_Tags.Unavailable
            ? "Inspection unavailable"
            : "Reset required"}
        </Badge>
        <BodyText>
          {localHost.tag === Bindings.LocalHostState_Tags.Unavailable
            ? localHost.inner.detail
            : localHost.inner.reason}
        </BodyText>
      </Card>
    );
  }

  const host = localHost.inner.host;
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
  { readonly tag: "Running" }
>["inner"]["host"]["destinationIdentities"][number];

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
    outcome?.tag === Bindings.ContactMutationOutcome_Tags.Saved ||
    outcome?.tag === Bindings.ContactMutationOutcome_Tags.Updated ||
    outcome?.tag === Bindings.ContactMutationOutcome_Tags.Existing;

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
  switch (outcome.tag) {
    case Bindings.ContactMutationOutcome_Tags.Saved:
      return "The verified destination was saved.";
    case Bindings.ContactMutationOutcome_Tags.Updated:
      return "The verified identity was added to the saved contact.";
    case Bindings.ContactMutationOutcome_Tags.Existing:
      return "This verified destination was already saved.";
    case Bindings.ContactMutationOutcome_Tags.IdentityConflict:
      return `The saved identity ${formatBytes(outcome.inner.existing)} differs from the observation ${formatBytes(outcome.inner.attempted)}.`;
    case Bindings.ContactMutationOutcome_Tags.NotObserved:
      return "This destination is no longer visible on the network.";
    case Bindings.ContactMutationOutcome_Tags.LocalNodeStopped:
      return "This device's node stopped before this address could be saved.";
    case Bindings.ContactMutationOutcome_Tags.DevelopmentUnavailable:
      return outcome.inner.detail;
    case Bindings.ContactMutationOutcome_Tags.DevelopmentResetRequired:
      return `Development reset required: ${outcome.inner.reason}`;
    case Bindings.ContactMutationOutcome_Tags.AlreadyExists:
      return "This destination is already saved.";
    case Bindings.ContactMutationOutcome_Tags.MissingIdentity:
      return "The destination does not have an authenticated identity.";
    case Bindings.ContactMutationOutcome_Tags.Deleted:
      return "The contact was deleted.";
    case Bindings.ContactMutationOutcome_Tags.NotFound:
      return "The contact was not found.";
  }
}
