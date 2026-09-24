import * as Bindings from "@prns-internal/expo";
import { useFocusEffect } from "expo-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { AppState } from "react-native";

import type {
  DevelopmentRuntimeView,
  RuntimeCommandResult,
} from "@/native/development-runtime-context";
import { BodyText, Button, Card, CardHeader, CardSection } from "@/ui/primitives";
import { TextField } from "@/ui/text-field";
import { formatBytes } from "../format";
import { remoteManagementFailureMessage } from "./change-status";
import { ConfirmAction } from "./confirm-action";

const requests = [
  Bindings.RemoteControlRequestKind.StageWifiCredentials,
  Bindings.RemoteControlRequestKind.ActivateWifiCredentials,
  Bindings.RemoteControlRequestKind.InspectWifiTransaction,
  Bindings.RemoteControlRequestKind.ConfirmWifiCredentials,
  Bindings.RemoteControlRequestKind.CancelWifiCredentials,
];

export function supportsWifiSetup(available: readonly Bindings.RemoteControlRequestKind[]) {
  return requests.every((request) => available.includes(request));
}

/** Only nonsecret native observations survive the form's lifetime. */
export function WifiSetupCard({
  target,
  runtime,
  busy,
}: {
  readonly target: Uint8Array;
  readonly runtime: DevelopmentRuntimeView;
  readonly busy: boolean;
}) {
  const [ssid, setSsid] = useState("");
  const [password, setPassword] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [failure, setFailure] = useState<string>();
  const [uncertain, setUncertain] = useState(false);
  const [accepted, setAccepted] = useState<Bindings.RemoteWifiOperation>();
  const [commandId, setCommandId] = useState<bigint>();
  const [focusRevision, setFocusRevision] = useState(0);
  const focused = useRef(false);
  const appActive = useRef(
    AppState.currentState !== "background" && AppState.currentState !== "inactive",
  );
  const epoch = useRef(0);
  const inFlight = useRef(false);
  const needsInspection = useRef(false);
  const targetId = formatBytes(target);
  const generationId = runtime.snapshot?.generationId;

  useFocusEffect(
    // biome-ignore lint/correctness/useExhaustiveDependencies: a target/native lifetime change invalidates the focused observation.
    useCallback(() => {
      focused.current = true;
      epoch.current += 1;
      needsInspection.current = true;
      setCommandId(undefined);
      setSubmitting(false);
      setFocusRevision((revision) => revision + 1);
      return () => {
        focused.current = false;
        epoch.current += 1;
        needsInspection.current = false;
        setPassword("");
        setSsid("");
      };
    }, [targetId, generationId]),
  );

  useEffect(() => {
    const subscription = AppState.addEventListener("change", (state) => {
      const active = state === "active";
      if (active === appActive.current) return;
      appActive.current = active;
      epoch.current += 1;
      setCommandId(undefined);
      setSubmitting(false);
      setPassword("");
      setSsid("");
      needsInspection.current = active && focused.current;
      setFocusRevision((revision) => revision + 1);
    });
    return () => subscription.remove();
  }, []);

  const stored = runtime.snapshot?.lastRemoteWifi;
  const current =
    stored !== undefined &&
    stored.generationId === generationId &&
    formatBytes(stored.targetIdentityFingerprint) === targetId
      ? stored
      : undefined;
  useEffect(() => {
    if (current !== undefined) {
      setAccepted((previous) =>
        previous === undefined || current.operationId >= previous.operationId ? current : previous,
      );
    }
  }, [current]);
  const latest =
    current !== undefined && (accepted === undefined || current.operationId >= accepted.operationId)
      ? current
      : accepted;
  const relevant =
    latest !== undefined &&
    latest.generationId === generationId &&
    formatBytes(latest.targetIdentityFingerprint) === targetId
      ? latest
      : undefined;
  const operation =
    relevant?.status.tag === Bindings.RemoteWifiStatus_Tags.Pending &&
    stored !== undefined &&
    stored.operationId > relevant.operationId
      ? {
          ...relevant,
          status: Bindings.RemoteWifiStatus.OutcomeUnknown.new({
            reason: Bindings.RemoteControlAnnounceUnknownReason.DeliveryUnconfirmed,
          }),
        }
      : relevant;
  const pending = operation?.status.tag === Bindings.RemoteWifiStatus_Tags.Pending;
  const blocked = busy || submitting || pending || runtime.snapshot?.activeOperation !== undefined;
  const fresh =
    !uncertain &&
    operation !== undefined &&
    operation.operationId === commandId &&
    operation.status.tag === Bindings.RemoteWifiStatus_Tags.Observed;
  const transaction =
    operation?.status.tag === Bindings.RemoteWifiStatus_Tags.Observed
      ? operation.status.inner.transaction
      : undefined;

  const command = useCallback(
    async (invoke: () => Promise<RuntimeCommandResult<Bindings.RemoteWifiCommandOutcome>>) => {
      if (
        !focused.current ||
        !appActive.current ||
        inFlight.current ||
        blocked ||
        runtime.snapshot?.runtime !== Bindings.DevelopmentNodeRuntime.Running
      )
        return;
      inFlight.current = true;
      setSubmitting(true);
      setFailure(undefined);
      setCommandId(undefined);
      const commandEpoch = epoch.current;
      // Invoke immediately, then clear the form. Never save credentials alongside
      // operation metadata or attempt to replay this closure after interruption.
      const resultPromise = invoke();
      setPassword("");
      setSsid("");
      const result = await resultPromise;
      inFlight.current = false;
      if (!focused.current || epoch.current !== commandEpoch) {
        // A refocused view may be waiting for this bridge call to release its
        // local guard. Wake its one deferred inspection, never replay a write.
        if (focused.current) setFocusRevision((revision) => revision + 1);
        return;
      }
      setSubmitting(false);
      if (result.type === "operationFailure") {
        setUncertain(true);
        await runtime.refreshSnapshot();
      } else if (result.outcome.tag === Bindings.RemoteWifiCommandOutcome_Tags.Accepted) {
        setAccepted(result.outcome.inner.operation);
        setCommandId(result.outcome.inner.operation.operationId);
        setUncertain(false);
        await runtime.refreshSnapshot();
      } else {
        setFailure(
          result.outcome.tag === Bindings.RemoteWifiCommandOutcome_Tags.Busy
            ? "Another operation is in progress. Check the network status before trying again."
            : remoteManagementFailureMessage(result.outcome.inner.stage),
        );
      }
    },
    [blocked, runtime],
  );

  const inspect = useCallback(
    () => command(() => runtime.inspectRemoteWifiTrial({ targetIdentityFingerprint: target })),
    [command, runtime, target],
  );

  useEffect(() => {
    if (
      focusRevision === 0 ||
      !focused.current ||
      !appActive.current ||
      !needsInspection.current ||
      blocked ||
      inFlight.current
    )
      return;
    needsInspection.current = false;
    void inspect();
  }, [blocked, focusRevision, inspect]);

  const awaiting =
    transaction?.tag === Bindings.RemoteWifiTransaction_Tags.AwaitingConfirmation
      ? transaction.inner
      : undefined;
  const staged =
    transaction?.tag === Bindings.RemoteWifiTransaction_Tags.Staged ? transaction.inner : undefined;
  const revision = awaiting?.revision ?? staged?.revision;
  const canEnterNetwork =
    fresh &&
    (transaction?.tag === Bindings.RemoteWifiTransaction_Tags.FactoryProvisioning ||
      transaction?.tag === Bindings.RemoteWifiTransaction_Tags.Confirmed);
  const validInput = utf8Length(ssid) > 0 && utf8Length(ssid) <= 32 && utf8Length(password) <= 64;
  const operationFailure =
    operation?.status.tag === Bindings.RemoteWifiStatus_Tags.Failed
      ? operation.action === Bindings.RemoteWifiAction.Keep &&
        operation.status.inner.stage === Bindings.RemoteManagementFailureStage.Busy
        ? "The node is not ready to keep this network. Check that it connected to Wi-Fi, then check the network status again."
        : remoteManagementFailureMessage(operation.status.inner.stage)
      : undefined;
  const outcomeUnknown =
    uncertain || operation?.status.tag === Bindings.RemoteWifiStatus_Tags.OutcomeUnknown;

  return (
    <Card>
      <CardHeader title="Wi-Fi network">
        <Button
          disabled={blocked}
          tone="secondary"
          accessibilityLabel="Check network status"
          onPress={() => void inspect()}
        >
          Check status
        </Button>
      </CardHeader>
      <BodyText muted>Unconfirmed trials automatically restore the previous network.</BodyText>
      {submitting || pending ? <BodyText>Waiting for the node…</BodyText> : null}
      {!fresh && !submitting && !pending ? (
        <BodyText muted>Check the node's network status before making a change.</BodyText>
      ) : null}
      {(failure ?? operationFailure) ? <BodyText>{failure ?? operationFailure}</BodyText> : null}
      {outcomeUnknown ? (
        <BodyText>
          The result could not be confirmed. No change has been repeated. Check the network status
          to continue.
        </BodyText>
      ) : null}
      {fresh && transaction !== undefined ? (
        <BodyText>{wifiTransactionMessage(transaction)}</BodyText>
      ) : null}
      {canEnterNetwork ? (
        <CardSection title="Try a network">
          <TextField
            label="Network name"
            value={ssid}
            onChangeText={setSsid}
            editable={!blocked}
            autoCapitalize="none"
            autoCorrect={false}
            autoComplete="off"
          />
          <TextField
            label="Network password"
            value={password}
            onChangeText={setPassword}
            editable={!blocked}
            secureTextEntry
            autoCapitalize="none"
            autoCorrect={false}
            autoComplete="off"
          />
          {utf8Length(ssid) > 32 ? (
            <BodyText>The network name is too long. Use at most 32 bytes of text.</BodyText>
          ) : null}
          {utf8Length(password) > 64 ? (
            <BodyText>The network password is too long. Use at most 64 bytes of text.</BodyText>
          ) : null}
          <ConfirmAction
            label="Try network"
            confirmationLabel="Start network trial"
            warning="The node will try this network. Your connection may be interrupted. Keep another way to reach the node available; you will have about two minutes to keep the network or restore the previous one."
            disabled={blocked || !validInput}
            onConfirm={() =>
              void command(() =>
                runtime.startRemoteWifiTrial({ targetIdentityFingerprint: target, ssid, password }),
              )
            }
          />
        </CardSection>
      ) : null}
      {fresh && awaiting !== undefined ? (
        <>
          {awaiting.remainingSeconds === 0 ? (
            <BodyText>Check network status to see whether this trial can still be saved.</BodyText>
          ) : (
            <BodyText>
              At the last check, about {awaiting.remainingSeconds} seconds remained to keep this
              network. Check again for the latest status.
            </BodyText>
          )}
          <ConfirmAction
            key={`keep:${awaiting.revision}`}
            label="Keep this network"
            confirmationLabel="Save network"
            warning="Only save this network after checking the node can use it. A working Bluetooth connection does not confirm that Wi-Fi works."
            disabled={blocked || awaiting.remainingSeconds === 0}
            onConfirm={() =>
              void command(() =>
                runtime.finishRemoteWifiTrial({
                  targetIdentityFingerprint: target,
                  revision: awaiting.revision,
                  decision: Bindings.RemoteWifiDecision.Keep,
                }),
              )
            }
          />
        </>
      ) : null}
      {fresh && revision !== undefined ? (
        <ConfirmAction
          key={`restore:${revision}`}
          label="Restore previous network"
          confirmationLabel="Restore network"
          warning="Stop trying this network and restore the node's previous network? This may briefly interrupt your connection."
          disabled={blocked}
          onConfirm={() =>
            void command(() =>
              runtime.finishRemoteWifiTrial({
                targetIdentityFingerprint: target,
                revision,
                decision: Bindings.RemoteWifiDecision.Restore,
              }),
            )
          }
        />
      ) : null}
    </Card>
  );
}

/** Count UTF-8 bytes rather than UTF-16 units; Rust still validates the input. */
export function utf8Length(value: string): number {
  let length = 0;
  for (const character of value) {
    const point = character.codePointAt(0) ?? 0;
    length += point <= 0x7f ? 1 : point <= 0x7ff ? 2 : point <= 0xffff ? 3 : 4;
  }
  return length;
}

export function wifiTransactionMessage(transaction: Bindings.RemoteWifiTransaction): string {
  switch (transaction.tag) {
    case Bindings.RemoteWifiTransaction_Tags.FactoryProvisioning:
      return "Ready to try a Wi-Fi network.";
    case Bindings.RemoteWifiTransaction_Tags.Confirmed:
      return "The node has a saved network. You can try a different one.";
    case Bindings.RemoteWifiTransaction_Tags.Staged:
      return "A network change was prepared but has not started. Restore the previous network before trying again.";
    case Bindings.RemoteWifiTransaction_Tags.AwaitingConfirmation:
      return "The node is trying a network and is waiting for your decision. This does not yet confirm that the network works.";
    case Bindings.RemoteWifiTransaction_Tags.RollingBack:
      return "The node is restoring its previous network. Check again to confirm it has finished.";
  }
}
