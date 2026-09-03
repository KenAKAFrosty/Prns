import type { RemoteControlDescribeOutcome } from "@prns-internal/expo";
import { useLocalSearchParams } from "expo-router";
import { useState } from "react";

import type { RuntimeCommandResult } from "@/native/development-runtime-context";
import { useDevelopmentRuntime } from "@/native/development-runtime-context";
import { type RawRouteParams, routeParamsAreValid, screenById } from "@/navigation/catalog";
import { NavigationLink } from "@/ui/navigation-link";
import {
  Badge,
  BodyText,
  Button,
  Card,
  KeyValue,
  Screen,
  ScreenHeading,
  Subheading,
} from "@/ui/primitives";
import { NotFoundScreen } from "../placeholder-screen";
import { formatBytes, formatRequestKind } from "./format";

export function ManagedNodeScreen() {
  const entry = screenById("nodes.managed");
  const params: RawRouteParams = useLocalSearchParams();
  const nodeId = params.nodeId;
  const runtime = useDevelopmentRuntime();
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<RuntimeCommandResult<RemoteControlDescribeOutcome> | null>(
    null,
  );

  if (!routeParamsAreValid(entry, params) || typeof nodeId !== "string") {
    return <NotFoundScreen backPath="/nodes" />;
  }

  if (runtime.phase === "unavailable") {
    return (
      <Screen>
        <Badge tone="warning">Node management unavailable</Badge>
        <ScreenHeading>Manage node</ScreenHeading>
        <BodyText>
          Node management is not available on {runtime.availability.platform} yet.
        </BodyText>
        <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
      </Screen>
    );
  }

  if (runtime.phase === "starting") {
    return (
      <Screen>
        <Badge>Getting ready</Badge>
        <ScreenHeading>Manage node</ScreenHeading>
        <BodyText>Loading node details…</BodyText>
        <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
      </Screen>
    );
  }

  if (runtime.phase === "failed") {
    return (
      <Screen>
        <Badge tone="warning">This device&apos;s node failed</Badge>
        <ScreenHeading>Manage node</ScreenHeading>
        <BodyText>
          This device&apos;s node could not start. Open its diagnostics for more details.
        </BodyText>
        <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
      </Screen>
    );
  }

  const target = runtime.snapshot?.pairedTargets.find(
    (candidate) => formatBytes(candidate.targetIdentityFingerprint) === nodeId.toLowerCase(),
  );
  if (target === undefined) {
    return <NotFoundScreen backPath="/nodes" />;
  }

  const canDescribe = target.permittedRequests.includes("describe");
  const describing = runtime.snapshot?.activeOperation?.kind === "describe";

  const describe = async () => {
    setResult(null);
    setPending(true);
    const next = await runtime.describeTarget({
      targetIdentityFingerprint: target.targetIdentityFingerprint,
    });
    setResult(next);
    setPending(false);
  };

  return (
    <Screen>
      <Badge>Paired node</Badge>
      <ScreenHeading>Manage node</ScreenHeading>
      <Card>
        <Subheading>Node details</Subheading>
        <KeyValue label="Node ID" value={formatBytes(target.targetIdentityFingerprint)} />
        <KeyValue label="Destination" value={formatBytes(target.destination)} />
        <KeyValue label="Controller" value={formatBytes(target.controllerIdentityFingerprint)} />
        <KeyValue
          label="Available actions"
          value={
            target.permittedRequests.length === 0
              ? "None"
              : target.permittedRequests.map(formatRequestKind).join(", ")
          }
        />
      </Card>
      <Card>
        <Subheading>Connection</Subheading>
        <KeyValue label="Status" value={describing ? "Checking node…" : "Ready to check"} />
        {canDescribe ? null : (
          <BodyText>This pairing does not allow the app to view node information.</BodyText>
        )}
        <Button disabled={!canDescribe || pending || describing} onPress={() => void describe()}>
          {pending || describing ? "Checking…" : "Check node connection"}
        </Button>
      </Card>

      {result === null ? null : <DescribeResult result={result} />}
      <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
    </Screen>
  );
}

function DescribeResult({
  result,
}: {
  readonly result: RuntimeCommandResult<RemoteControlDescribeOutcome>;
}) {
  if (result.type === "operationFailure") {
    return (
      <Card>
        <Subheading>Connection failed</Subheading>
        <Badge tone="warning">Could not reach node</Badge>
        <BodyText>Something went wrong while checking the node. Try again.</BodyText>
      </Card>
    );
  }
  switch (result.outcome.type) {
    case "busy":
      return (
        <Card>
          <Subheading>Connection not started</Subheading>
          <Badge tone="warning">Another operation is in progress</Badge>
          <BodyText>Wait for the other operation to finish, then try again.</BodyText>
        </Card>
      );
    case "failed":
      return (
        <Card>
          <Subheading>Connection failed</Subheading>
          <Badge tone="warning">Could not check node</Badge>
          <BodyText>{describeFailureMessage(result.outcome.stage)}</BodyText>
        </Card>
      );
    case "described":
      return (
        <Card>
          <Subheading>Node reached</Subheading>
          <Badge>Connected</Badge>
          <KeyValue label="Response time" value={`${result.outcome.rttMillis.toString()} ms`} />
          <KeyValue
            label="Available actions"
            value={
              result.outcome.availableRequests.length === 0
                ? "None"
                : result.outcome.availableRequests.map(formatRequestKind).join(", ")
            }
          />
          <KeyValue
            label="Node ID"
            value={formatBytes(result.outcome.target.targetIdentityFingerprint)}
          />
          <KeyValue label="Destination" value={formatBytes(result.outcome.target.destination)} />
          <KeyValue
            label="Controller"
            value={formatBytes(result.outcome.target.controllerIdentityFingerprint)}
          />
        </Card>
      );
  }
}

type DescribeFailureStage = Extract<
  RemoteControlDescribeOutcome,
  { readonly type: "failed" }
>["stage"];

export function describeFailureMessage(stage: DescribeFailureStage): string {
  switch (stage) {
    case "input":
    case "inventory":
      return "This paired node is no longer available.";
    case "route":
      return "This node is not reachable yet. Keep it on and nearby, then try again.";
    case "link":
      return "A secure connection to this node could not be opened. Try again.";
    case "identification":
      return "The saved pairing could not be used with this node. Check that it is still paired, then try again.";
    case "request":
      return "The node could not complete the connection check. Try again.";
    case "timeout":
      return "The node did not answer before the connection check timed out. Make sure it is on and nearby, then try again.";
    case "permission":
      return "This pairing does not allow the app to view node information.";
    case "node":
      return "This device went offline before the check finished.";
  }
}
