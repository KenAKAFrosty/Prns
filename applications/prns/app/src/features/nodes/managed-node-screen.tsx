import type { RemoteControlDescribeOutcome } from "@prns-internal/expo";
import { useLocalSearchParams } from "expo-router";
import { useState } from "react";

import type { RuntimeCommandResult } from "@/native/development-runtime-context";
import { useDevelopmentRuntime } from "@/native/development-runtime-context";
import { screenById, type RawRouteParams, routeParamsAreValid } from "@/navigation/catalog";
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
        <Badge tone="warning">Native runtime unavailable</Badge>
        <ScreenHeading>Managed node</ScreenHeading>
        <BodyText>
          The {runtime.availability.platform} provider cannot load a persisted target inventory or
          issue Describe. No fixture result is shown.
        </BodyText>
        <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
      </Screen>
    );
  }

  if (runtime.phase === "starting") {
    return (
      <Screen>
        <Badge>Starting</Badge>
        <ScreenHeading>Managed node</ScreenHeading>
        <BodyText>Waiting for persistence restore before resolving the requested target.</BodyText>
        <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
      </Screen>
    );
  }

  if (runtime.phase === "failed") {
    return (
      <Screen>
        <Badge tone="warning">Local node failed</Badge>
        <ScreenHeading>Managed node</ScreenHeading>
        <BodyText>
          {runtime.lifecycleFailure ?? "The native provider did not return a reason."}
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
      <Badge>Persisted RemoteControl target</Badge>
      <ScreenHeading>Managed node</ScreenHeading>
      <Card>
        <Subheading>Target identity</Subheading>
        <KeyValue label="Fingerprint" value={formatBytes(target.targetIdentityFingerprint)} />
        <KeyValue label="Destination" value={formatBytes(target.destination)} />
        <KeyValue
          label="Controller fingerprint"
          value={formatBytes(target.controllerIdentityFingerprint)}
        />
        <KeyValue
          label="Permitted requests"
          value={
            target.permittedRequests.length === 0
              ? "None"
              : target.permittedRequests.map(formatRequestKind).join(", ")
          }
        />
      </Card>
      <Card>
        <Subheading>Connection and Describe</Subheading>
        <KeyValue
          label="Connection"
          value={
            describing
              ? "Opening target link and requesting Describe"
              : "Idle — no target link is retained"
          }
        />
        {canDescribe ? null : (
          <BodyText>This persisted authorization does not permit Describe.</BodyText>
        )}
        <Button disabled={!canDescribe || pending || describing} onPress={() => void describe()}>
          {pending || describing ? "Describing…" : "Connect and Describe"}
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
        <Subheading>Describe failed</Subheading>
        <Badge tone="warning">Native operation failed</Badge>
        <BodyText>{result.detail}</BodyText>
      </Card>
    );
  }
  switch (result.outcome.type) {
    case "busy":
      return (
        <Card>
          <Subheading>Describe not started</Subheading>
          <Badge tone="warning">Runtime busy</Badge>
          <BodyText>Another pairing or target operation is active.</BodyText>
        </Card>
      );
    case "failed":
      return (
        <Card>
          <Subheading>Describe failed</Subheading>
          <Badge tone="warning">{result.outcome.stage}</Badge>
          <BodyText>{result.outcome.detail}</BodyText>
        </Card>
      );
    case "described":
      return (
        <Card>
          <Subheading>Describe result</Subheading>
          <Badge>Real response received; link closed</Badge>
          <KeyValue label="Round-trip time" value={`${result.outcome.rttMillis.toString()} ms`} />
          <KeyValue
            label="Available requests"
            value={
              result.outcome.availableRequests.length === 0
                ? "None"
                : result.outcome.availableRequests.map(formatRequestKind).join(", ")
            }
          />
          <KeyValue
            label="Target fingerprint"
            value={formatBytes(result.outcome.target.targetIdentityFingerprint)}
          />
          <KeyValue label="Destination" value={formatBytes(result.outcome.target.destination)} />
          <KeyValue
            label="Controller fingerprint"
            value={formatBytes(result.outcome.target.controllerIdentityFingerprint)}
          />
        </Card>
      );
  }
}
