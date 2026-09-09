import * as Bindings from "@prns-internal/expo";
import type {
  RemoteControlDescribeOutcome,
  RemoteControlAnnounceOutcome,
  RemoteControlAnnounceStatus,
} from "@prns-internal/expo";
import { useFocusEffect, useLocalSearchParams } from "expo-router";
import { useCallback, useRef, useState } from "react";

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
  const [announcing, setAnnouncing] = useState(false);
  const [unknownSubmission, setUnknownSubmission] = useState<{
    targetId: string;
    previousOperationId: bigint | null;
  } | null>(null);
  const [announceResult, setAnnounceResult] =
    useState<RuntimeCommandResult<RemoteControlAnnounceOutcome> | null>(null);
  const [result, setResult] = useState<RuntimeCommandResult<RemoteControlDescribeOutcome> | null>(
    null,
  );
  const describeRequest = useRef<AbortController | null>(null);
  useFocusEffect(
    // biome-ignore lint/correctness/useExhaustiveDependencies: target and native lifetime changes invalidate the read even while this route stays focused.
    useCallback(() => {
      setPending(false);
      return () => {
        // Stack routes can stay mounted while another page is visible. Release
        // this read on blur, target/generation change, and unmount alike.
        describeRequest.current?.abort();
        describeRequest.current = null;
      };
    }, [nodeId, runtime.phase, runtime.snapshot?.generationId, runtime.snapshot?.runtime]),
  );

  if (
    !routeParamsAreValid(entry, params) ||
    typeof nodeId !== "string" ||
    !/^[0-9a-fA-F]{32}$/u.test(nodeId)
  ) {
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

  if (runtime.snapshot?.runtime !== Bindings.DevelopmentNodeRuntime.Running) {
    const nodeState = runtime.snapshot?.runtime;
    const android = runtime.availability.platform === "android";
    const guidance = {
      [Bindings.DevelopmentNodeRuntime.Stopped]: {
        title: "This device's node is stopped",
        detail: android
          ? "Return to Nodes and start this device's node to manage your paired nodes."
          : "Return to Nodes to check this device's status.",
      },
      [Bindings.DevelopmentNodeRuntime.Stopping]: {
        title: "This device's node is stopping",
        detail: android
          ? "Wait for it to stop, then return to Nodes to start it again."
          : "Wait for it to stop, then return to Nodes to check this device's status.",
      },
      [Bindings.DevelopmentNodeRuntime.Starting]: {
        title: "This device's node is starting",
        detail: "Your paired node's details will appear when this device is ready.",
      },
      [Bindings.DevelopmentNodeRuntime.Failed]: {
        title: "This device's node is unavailable",
        detail: android
          ? "Return to Nodes to check this device and try starting it again."
          : "Return to Nodes to check this device's status.",
      },
      unavailable: {
        title: "Node details unavailable",
        detail: "Return to Nodes to check this device's status.",
      },
    }[nodeState ?? "unavailable"];
    return (
      <Screen>
        <Badge tone="warning">{guidance.title}</Badge>
        <ScreenHeading>Manage node</ScreenHeading>
        <BodyText>{guidance.detail}</BodyText>
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

  const nodeRunning = runtime.snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Running;
  const allowsDescribe = target.permittedRequests.includes(
    Bindings.RemoteControlRequestKind.Describe,
  );
  const canDescribe = nodeRunning && allowsDescribe;
  const describing =
    runtime.snapshot?.activeOperation?.kind === Bindings.DevelopmentNodeOperationKind.Describe;
  const announcement = runtime.snapshot?.lastAnnouncement;
  const targetAnnouncement =
    announcement !== undefined &&
    formatBytes(announcement.targetIdentityFingerprint) === nodeId.toLowerCase()
      ? announcement
      : null;
  const announcementPending =
    targetAnnouncement?.status?.tag === Bindings.RemoteControlAnnounceStatus_Tags.Pending;
  const canAnnounce =
    nodeRunning &&
    result?.type === "outcome" &&
    result.outcome.tag === Bindings.RemoteControlDescribeOutcome_Tags.Described &&
    formatBytes(result.outcome.inner.target.targetIdentityFingerprint) === nodeId.toLowerCase() &&
    result.outcome.inner.snapshot.generationId === runtime.snapshot?.generationId &&
    target.permittedRequests.includes(Bindings.RemoteControlRequestKind.AnnounceSelf) &&
    result.outcome.inner.availableRequests.includes(Bindings.RemoteControlRequestKind.AnnounceSelf);
  const operationBusy = runtime.snapshot?.activeOperation !== undefined;
  const admissionUncertain =
    unknownSubmission?.targetId === nodeId.toLowerCase() &&
    (targetAnnouncement === null ||
      targetAnnouncement.operationId === unknownSubmission.previousOperationId);

  const describe = async () => {
    if (describeRequest.current !== null) return;
    const request = new AbortController();
    describeRequest.current = request;
    setResult(null);
    setPending(true);
    const next = await runtime.describeTarget(
      { targetIdentityFingerprint: target.targetIdentityFingerprint },
      request.signal,
    );
    if (describeRequest.current !== request) return;
    describeRequest.current = null;
    setPending(false);
    if (!request.signal.aborted) setResult(next);
  };

  const announce = async () => {
    const previousOperationId = targetAnnouncement?.operationId ?? null;
    setAnnouncing(true);
    setAnnounceResult(null);
    setUnknownSubmission(null);
    const next = await runtime.announceTarget({
      targetIdentityFingerprint: target.targetIdentityFingerprint,
    });
    setAnnounceResult(next);
    // A bridge interruption can lose the admission reply. Refresh the native
    // record; never submit again to infer whether the first request was accepted.
    if (next.type === "operationFailure") {
      setUnknownSubmission({ targetId: nodeId.toLowerCase(), previousOperationId });
      await runtime.refreshSnapshot();
    }
    setAnnouncing(false);
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
        <KeyValue
          label="Status"
          value={
            !nodeRunning
              ? "This device is offline"
              : describing
                ? "Checking node…"
                : "Ready to check"
          }
        />
        {allowsDescribe ? null : (
          <BodyText>This pairing does not allow the app to view node information.</BodyText>
        )}
        <Button
          disabled={!canDescribe || pending || operationBusy || announcing}
          onPress={() => void describe()}
        >
          {pending || describing ? "Checking…" : "Check node connection"}
        </Button>
      </Card>

      {result === null ? null : <DescribeResult result={result} />}
      {canAnnounce ? (
        <Card>
          <Subheading>Share node address</Subheading>
          <BodyText>Ask this node to make itself discoverable on its network.</BodyText>
          <Button
            disabled={
              announcing || pending || operationBusy || announcementPending || admissionUncertain
            }
            onPress={() => void announce()}
          >
            {announcing || announcementPending ? "Sharing…" : "Share node address"}
          </Button>
        </Card>
      ) : null}
      {targetAnnouncement === null ? null : (
        <Card>
          <Subheading>
            {admissionUncertain ? "Previous address sharing" : "Address sharing"}
          </Subheading>
          <BodyText>{announcementStatusMessage(targetAnnouncement.status)}</BodyText>
          {targetAnnouncement.status.tag === Bindings.RemoteControlAnnounceStatus_Tags.Announced ? (
            <KeyValue
              label="Response time"
              value={`${targetAnnouncement.status.inner.rttMillis.toString()} ms`}
            />
          ) : null}
        </Card>
      )}
      {admissionUncertain ? (
        <BodyText>
          The result could not be confirmed. The node may have shared its address. This request was
          not repeated.
        </BodyText>
      ) : announceResult?.type === "outcome" &&
        announceResult.outcome.tag === Bindings.RemoteControlAnnounceOutcome_Tags.Busy ? (
        <BodyText>Another operation is in progress. Address sharing has not started.</BodyText>
      ) : announceResult?.type === "outcome" &&
        announceResult.outcome.tag === Bindings.RemoteControlAnnounceOutcome_Tags.Failed ? (
        <BodyText>{announcementFailureMessage(announceResult.outcome.inner.stage)}</BodyText>
      ) : null}
      <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
    </Screen>
  );
}

export function announcementStatusMessage(status: RemoteControlAnnounceStatus): string {
  switch (status.tag) {
    case Bindings.RemoteControlAnnounceStatus_Tags.Pending:
      return "Waiting for the node to confirm address sharing…";
    case Bindings.RemoteControlAnnounceStatus_Tags.Announced:
      return "The node confirmed that it shared its address.";
    case Bindings.RemoteControlAnnounceStatus_Tags.Unavailable:
      return "The node cannot share its address right now.";
    case Bindings.RemoteControlAnnounceStatus_Tags.Rejected:
      return "The node declined to share its address.";
    case Bindings.RemoteControlAnnounceStatus_Tags.WriteFailed:
      return "The node could not send its address announcement.";
    case Bindings.RemoteControlAnnounceStatus_Tags.OutcomeUnknown:
      return "The result could not be confirmed. The node may have shared its address. This request was not repeated.";
    case Bindings.RemoteControlAnnounceStatus_Tags.Failed:
      return announcementFailureMessage(status.inner.stage);
  }
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
  switch (result.outcome.tag) {
    case Bindings.RemoteControlDescribeOutcome_Tags.Busy:
      return (
        <Card>
          <Subheading>Connection not started</Subheading>
          <Badge tone="warning">Another operation is in progress</Badge>
          <BodyText>Wait for the other operation to finish, then try again.</BodyText>
        </Card>
      );
    case Bindings.RemoteControlDescribeOutcome_Tags.Failed:
      return (
        <Card>
          <Subheading>Connection failed</Subheading>
          <Badge tone="warning">Could not check node</Badge>
          <BodyText>{describeFailureMessage(result.outcome.inner.stage)}</BodyText>
        </Card>
      );
    case Bindings.RemoteControlDescribeOutcome_Tags.Described:
      return (
        <Card>
          <Subheading>Node reached</Subheading>
          <Badge>Connected</Badge>
          <KeyValue
            label="Response time"
            value={`${result.outcome.inner.rttMillis.toString()} ms`}
          />
          <KeyValue
            label="Available actions"
            value={
              result.outcome.inner.availableRequests.length === 0
                ? "None"
                : result.outcome.inner.availableRequests.map(formatRequestKind).join(", ")
            }
          />
          <KeyValue
            label="Node ID"
            value={formatBytes(result.outcome.inner.target.targetIdentityFingerprint)}
          />
          <KeyValue
            label="Destination"
            value={formatBytes(result.outcome.inner.target.destination)}
          />
          <KeyValue
            label="Controller"
            value={formatBytes(result.outcome.inner.target.controllerIdentityFingerprint)}
          />
        </Card>
      );
  }
}

type DescribeFailureStage = Extract<
  RemoteControlDescribeOutcome,
  { readonly tag: "Failed" }
>["inner"]["stage"];

export function describeFailureMessage(stage: DescribeFailureStage): string {
  switch (stage) {
    case Bindings.RemoteControlDescribeFailureStage.Input:
    case Bindings.RemoteControlDescribeFailureStage.Inventory:
      return "This paired node is no longer available.";
    case Bindings.RemoteControlDescribeFailureStage.Route:
      return "This node is not reachable yet. Keep it on and nearby, then try again.";
    case Bindings.RemoteControlDescribeFailureStage.Link:
      return "The connection check could not complete. Try again.";
    case Bindings.RemoteControlDescribeFailureStage.Identification:
      return "The saved pairing could not be used with this node. Check that it is still paired, then try again.";
    case Bindings.RemoteControlDescribeFailureStage.Request:
      return "The node could not complete the connection check. Try again.";
    case Bindings.RemoteControlDescribeFailureStage.Timeout:
      return "The node did not answer before the connection check timed out. Make sure it is on and nearby, then try again.";
    case Bindings.RemoteControlDescribeFailureStage.Permission:
      return "This pairing does not allow the app to view node information.";
    case Bindings.RemoteControlDescribeFailureStage.Node:
      return "This device went offline before the check finished.";
  }
}

function announcementFailureMessage(stage: Bindings.RemoteControlAnnounceFailureStage): string {
  switch (stage) {
    case Bindings.RemoteControlAnnounceFailureStage.Busy:
      return "Another operation is in progress. Address sharing has not started.";
    case Bindings.RemoteControlAnnounceFailureStage.Input:
    case Bindings.RemoteControlAnnounceFailureStage.Inventory:
      return "This paired node is no longer available.";
    case Bindings.RemoteControlAnnounceFailureStage.Route:
    case Bindings.RemoteControlAnnounceFailureStage.Link:
      return "The node could not be reached. Keep it on and nearby.";
    case Bindings.RemoteControlAnnounceFailureStage.Identification:
      return "The saved pairing could not be used with this node.";
    case Bindings.RemoteControlAnnounceFailureStage.Permission:
      return "This pairing does not allow the node to share its address.";
    case Bindings.RemoteControlAnnounceFailureStage.Node:
      return "This device went offline before address sharing could start.";
    case Bindings.RemoteControlAnnounceFailureStage.Request:
      return "Address sharing could not start.";
  }
}
