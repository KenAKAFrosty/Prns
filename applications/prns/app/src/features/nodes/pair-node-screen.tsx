import type {
  DevelopmentNodeSnapshot,
  RemoteControlPairingCommandOutcome,
} from "@prns-internal/expo";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { StyleSheet, TextInput } from "react-native";

import type { RuntimeCommandResult } from "@/native/development-runtime-context";
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
import { radius, space, useAppPalette } from "@/ui/theme";
import { formatBytes, formatRequestKind } from "./format";

type RemoteControlPairingState = DevelopmentNodeSnapshot["pairing"];
type RemoteControlPairingCandidate = DevelopmentNodeSnapshot["pairingCandidates"][number];

export function PairNodeScreen({
  selectedCandidateId,
}: {
  readonly selectedCandidateId: string | undefined;
}) {
  const runtime = useDevelopmentRuntime();
  const palette = useAppPalette();
  const [invitationCode, setInvitationCode] = useState("");
  const [pending, setPending] = useState<"approve" | "initiate" | "reject" | null>(null);
  const [setupPending, setSetupPending] = useState(false);
  const [setupFeedback, setSetupFeedback] = useState<string | null>(null);
  const [commandFailure, setCommandFailure] = useState<string | null>(null);
  const [candidateId, setCandidateId] = useState<string | undefined>(selectedCandidateId);
  const pairing = runtime.snapshot?.pairing;
  const candidates = runtime.snapshot?.pairingCandidates ?? [];
  const selectedCandidateIsPresent = candidates.some(
    (candidate) => candidate.candidateId === candidateId,
  );
  const candidateSelectionKey = `${candidateId ?? "none"}:${selectedCandidateIsPresent ? "present" : "missing"}`;
  const previousCandidateSelectionKey = useRef(candidateSelectionKey);

  useEffect(() => {
    if (pairing?.type !== "searching") {
      setInvitationCode("");
    }
  }, [pairing?.type]);

  useEffect(() => {
    setCandidateId(selectedCandidateId);
  }, [selectedCandidateId]);

  useEffect(() => {
    setCandidateId((current) =>
      current === undefined && candidates.length === 1 ? candidates[0]?.candidateId : current,
    );
  }, [candidates]);

  useEffect(() => {
    if (!isTerminalPairingType(pairing?.type)) {
      return;
    }
    setCandidateId((current) => {
      if (
        current === undefined ||
        candidates.some((candidate) => candidate.candidateId === current)
      ) {
        return current;
      }
      return candidates.length === 1 ? candidates[0]?.candidateId : undefined;
    });
  }, [candidates, pairing?.type]);

  useEffect(() => {
    if (previousCandidateSelectionKey.current === candidateSelectionKey) {
      return;
    }
    previousCandidateSelectionKey.current = candidateSelectionKey;
    setInvitationCode("");
    setCommandFailure(null);
  }, [candidateSelectionKey]);

  const initiate = async (selected: RemoteControlPairingCandidate) => {
    setPending("initiate");
    setCommandFailure(null);
    const result = await runtime.initiatePairing({
      candidateId: selected.candidateId,
      invitationCode,
    });
    setCommandFailure(pairingCommandFailure(result));
    setPending(null);
  };

  const showAccessorySetup = async () => {
    setSetupPending(true);
    setSetupFeedback(null);
    const result = await runtime.showAccessorySetupPicker();
    if (result.type === "operationFailure") {
      setSetupFeedback("The Bluetooth chooser could not open. Try again.");
    } else {
      setSetupFeedback(pickerOutcomeCopy(result.outcome.type));
    }
    setSetupPending(false);
  };

  const decide = async (
    decision: "approve" | "reject",
    state: Extract<RemoteControlPairingState, { type: "confirmationRequired" }>,
  ) => {
    setPending(decision);
    setCommandFailure(null);
    const result = await (decision === "approve" ? runtime.approvePairing : runtime.rejectPairing)({
      attemptId: state.attemptId,
    });
    setCommandFailure(pairingCommandFailure(result));
    setPending(null);
  };

  return (
    <Screen>
      <Badge>Secure pairing</Badge>
      <ScreenHeading>Pair a node</ScreenHeading>
      <BodyText>
        First choose a nearby Bluetooth accessory. Then open secure pairing on the node you want to
        manage, enter its invitation, and compare the confirmation code on both devices.
      </BodyText>

      {runtime.availability.type === "available" ? (
        <AccessorySetupCard
          feedback={setupFeedback}
          onShow={() => void showAccessorySetup()}
          pending={setupPending}
          setup={runtime.accessorySetup}
          setupFailure={runtime.accessorySetupFailure}
        />
      ) : null}

      {runtime.phase === "unavailable" ? (
        <Card>
          <Subheading>Pairing unavailable</Subheading>
          <Badge tone="warning">Not supported on {runtime.availability.platform}</Badge>
          <BodyText>Node pairing is not available on this platform yet.</BodyText>
        </Card>
      ) : null}

      {runtime.phase === "starting" && runtime.accessorySetup?.phase === "ready" ? (
        <Card>
          <Subheading>Getting ready</Subheading>
          <Badge>Starting</Badge>
          <BodyText muted>Pairing will be available in a moment.</BodyText>
        </Card>
      ) : null}

      {runtime.phase === "failed" &&
      runtime.accessorySetup?.phase !== "failed" &&
      runtime.accessorySetupFailure === null ? (
        <Card>
          <Subheading>This device&apos;s node failed to start</Subheading>
          <Badge tone="warning">Pairing unavailable</Badge>
          <BodyText>
            Pairing cannot start because this device&apos;s node is unavailable. Open its
            diagnostics for more details.
          </BodyText>
        </Card>
      ) : null}

      {runtime.snapshot === null ? null : (
        <PairingStateCard
          commandFailure={commandFailure}
          candidates={candidates}
          invitationCode={invitationCode}
          onApprove={(state) => void decide("approve", state)}
          onInvitationCode={(value) =>
            setInvitationCode(
              value
                .replaceAll(/[^0-9a-f]/giu, "")
                .toUpperCase()
                .slice(0, 8),
            )
          }
          onInitiate={(selected) => void initiate(selected)}
          onReject={(state) => void decide("reject", state)}
          onSelectCandidate={setCandidateId}
          palette={palette}
          pairing={runtime.snapshot.pairing}
          pending={pending}
          selectedCandidateId={candidateId}
        />
      )}

      <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
    </Screen>
  );
}

function AccessorySetupCard({
  feedback,
  onShow,
  pending,
  setup,
  setupFailure,
}: {
  readonly feedback: string | null;
  readonly onShow: () => void;
  readonly pending: boolean;
  readonly setup: ReturnType<typeof useDevelopmentRuntime>["accessorySetup"];
  readonly setupFailure: string | null;
}) {
  if (setupFailure !== null) {
    return (
      <Card>
        <Subheading>Bluetooth access unavailable</Subheading>
        <Badge tone="warning">Status unavailable</Badge>
        <BodyText>prns could not check Bluetooth access. Relaunch the app to try again.</BodyText>
      </Card>
    );
  }
  if (setup === null || setup.phase === "activating") {
    return (
      <Card>
        <Subheading>Checking Bluetooth access</Subheading>
        <Badge>Getting ready</Badge>
        <BodyText muted>Checking Bluetooth access…</BodyText>
      </Card>
    );
  }
  if (setup.phase === "failed") {
    return (
      <Card>
        <Subheading>Bluetooth access unavailable</Subheading>
        <Badge tone="warning">Relaunch required</Badge>
        <BodyText>
          {setup.lastError?.detail ??
            "Bluetooth access checking stopped. Relaunch the app to try again."}
        </BodyText>
      </Card>
    );
  }

  const pickerOpen = setup.picker !== "idle" || pending;
  if (setup.phase === "setupRequired") {
    return (
      <Card>
        <Subheading>Choose a nearby node</Subheading>
        <Badge tone="warning">Bluetooth access needed</Badge>
        <BodyText>
          Use the Bluetooth chooser to allow prns to connect to a nearby Reticulum node. Secure
          Reticulum pairing happens next.
        </BodyText>
        {setup.nativeStart === "running" ? (
          <BodyText>
            Bluetooth access was removed while this device stayed running. Authorize a node again to
            reconnect.
          </BodyText>
        ) : null}
        {setup.lastError === null || feedback !== null ? null : (
          <BodyText>{setup.lastError.detail}</BodyText>
        )}
        {feedback === null ? null : <BodyText>{feedback}</BodyText>}
        <Button disabled={pickerOpen} onPress={onShow}>
          {pickerOpen ? "Bluetooth chooser open…" : "Choose a Bluetooth node"}
        </Button>
      </Card>
    );
  }

  return (
    <Card>
      <Subheading>Bluetooth access ready</Subheading>
      <Badge>Bluetooth access granted</Badge>
      <BodyText>
        {setup.authorizedAccessoryCount === 1
          ? "One Bluetooth node is available to prns. Continue with secure Reticulum pairing below."
          : `${setup.authorizedAccessoryCount} Bluetooth nodes are available to prns. Continue with secure Reticulum pairing below.`}
      </BodyText>
      {setup.lastError === null || feedback !== null ? null : (
        <BodyText>{setup.lastError.detail}</BodyText>
      )}
      {feedback === null ? null : <BodyText>{feedback}</BodyText>}
      <Button disabled={pickerOpen} onPress={onShow} tone="secondary">
        {pickerOpen ? "Bluetooth chooser open…" : "Add another Bluetooth node"}
      </Button>
    </Card>
  );
}

function pickerOutcomeCopy(
  outcome:
    | "alreadyActive"
    | "cancelled"
    | "completed"
    | "failed"
    | "notReady"
    | "restricted"
    | "timedOut",
): string | null {
  switch (outcome) {
    case "completed":
      return null;
    case "cancelled":
      return "The Bluetooth chooser was cancelled. No Bluetooth access changed.";
    case "timedOut":
      return "No matching Bluetooth node was found. Move it nearby and try again.";
    case "restricted":
      return "The Bluetooth chooser can only open while prns is in the foreground.";
    case "alreadyActive":
      return "The Bluetooth chooser is already open.";
    case "notReady":
      return "Bluetooth access is still getting ready.";
    case "failed":
      return "Bluetooth access could not be completed.";
  }
}

function PairingStateCard({
  candidates,
  commandFailure,
  invitationCode,
  onApprove,
  onInitiate,
  onInvitationCode,
  onReject,
  onSelectCandidate,
  palette,
  pairing,
  pending,
  selectedCandidateId,
}: {
  readonly candidates: readonly RemoteControlPairingCandidate[];
  readonly commandFailure: string | null;
  readonly invitationCode: string;
  readonly onApprove: (
    pairing: Extract<RemoteControlPairingState, { type: "confirmationRequired" }>,
  ) => void;
  readonly onInitiate: (candidate: RemoteControlPairingCandidate) => void;
  readonly onInvitationCode: (value: string) => void;
  readonly onReject: (
    pairing: Extract<RemoteControlPairingState, { type: "confirmationRequired" }>,
  ) => void;
  readonly onSelectCandidate: (candidateId: string) => void;
  readonly palette: ReturnType<typeof useAppPalette>;
  readonly pairing: RemoteControlPairingState;
  readonly pending: "approve" | "initiate" | "reject" | null;
  readonly selectedCandidateId: string | undefined;
}) {
  const feedback = commandFailure === null ? null : <BodyText>{commandFailure}</BodyText>;
  const remainingCandidateChooser =
    candidates.length === 0 ? null : (
      <CandidateSelectionCard
        candidates={candidates}
        commandFeedback={feedback}
        invitationCode={invitationCode}
        onInitiate={onInitiate}
        onInvitationCode={onInvitationCode}
        onSelectCandidate={onSelectCandidate}
        palette={palette}
        pending={pending}
        selectedCandidateId={selectedCandidateId}
      />
    );

  switch (pairing.type) {
    case "bluetoothUnavailable":
      return (
        <Card>
          <Subheading>Bluetooth unavailable</Subheading>
          <Badge tone="warning">Cannot search</Badge>
          <BodyText>Turn on Bluetooth and make sure prns has permission to use it.</BodyText>
          {feedback}
        </Card>
      );
    case "searching":
      return (
        <CandidateSelectionCard
          candidates={candidates}
          commandFeedback={feedback}
          invitationCode={invitationCode}
          onInitiate={onInitiate}
          onInvitationCode={onInvitationCode}
          onSelectCandidate={onSelectCandidate}
          palette={palette}
          pending={pending}
          selectedCandidateId={selectedCandidateId}
        />
      );
    case "invitationSubmitted":
      return (
        <Card>
          <Subheading>Invitation sent</Subheading>
          <Badge>Waiting for confirmation</Badge>
          <BodyText>Check both devices for a confirmation code.</BodyText>
          {feedback}
        </Card>
      );
    case "confirmationRequired":
      return (
        <Card>
          <Subheading>Confirmation required</Subheading>
          <Badge tone="warning">Compare both devices</Badge>
          <KeyValue label="Confirmation code" value={pairing.confirmationCode} />
          <KeyValue label="Node ID" value={formatBytes(pairing.targetIdentityFingerprint)} />
          <KeyValue
            label="Access requested"
            value={
              pairing.permissions.length === 0
                ? "None"
                : pairing.permissions.map(formatRequestKind).join(", ")
            }
          />
          <BodyText>
            If the codes match, approve on the node first, then approve here. Otherwise, reject
            pairing.
          </BodyText>
          <CardStack>
            <Button disabled={pending !== null} onPress={() => onApprove(pairing)}>
              {pending === "approve" ? "Approving…" : "Codes match — approve"}
            </Button>
            <Button
              disabled={pending !== null}
              onPress={() => onReject(pairing)}
              tone="destructive"
            >
              {pending === "reject" ? "Rejecting…" : "Reject pairing"}
            </Button>
          </CardStack>
          {feedback}
        </Card>
      );
    case "awaitingTargetApproval":
      return (
        <Card>
          <Subheading>Waiting for the node</Subheading>
          <Badge>Approve on the node</Badge>
          <BodyText>Approve pairing on the node to continue.</BodyText>
          {feedback}
        </Card>
      );
    case "persisting":
      return (
        <Card>
          <Subheading>Finishing pairing</Subheading>
          <Badge>Saving</Badge>
          <BodyText>Saving this connection on both devices.</BodyText>
          {feedback}
        </Card>
      );
    case "paired":
      return (
        <>
          <Card>
            <Subheading>Paired</Subheading>
            <Badge>Ready</Badge>
            <BodyText>This node is now available in Nodes.</BodyText>
            {candidates.length === 0 ? feedback : null}
          </Card>
          {remainingCandidateChooser}
        </>
      );
    case "rejected":
      return (
        <>
          <TerminalPairingCard
            detail="Pairing was declined on one of the devices."
            label="Rejected"
          />
          {remainingCandidateChooser}
        </>
      );
    case "expired":
      return (
        <>
          <TerminalPairingCard
            detail="The invitation expired. Reopen pairing on the node and try again."
            label="Expired"
          />
          {remainingCandidateChooser}
        </>
      );
    case "cancelled":
      return (
        <>
          <TerminalPairingCard detail="Pairing was cancelled." label="Cancelled" />
          {remainingCandidateChooser}
        </>
      );
    case "failed":
      return (
        <>
          <TerminalPairingCard
            detail={pairingFailureMessage(pairing.stage)}
            label="Pairing failed"
          />
          {remainingCandidateChooser}
        </>
      );
  }
}

function CandidateSelectionCard({
  candidates,
  commandFeedback,
  invitationCode,
  onInitiate,
  onInvitationCode,
  onSelectCandidate,
  palette,
  pending,
  selectedCandidateId,
}: {
  readonly candidates: readonly RemoteControlPairingCandidate[];
  readonly commandFeedback: ReactNode;
  readonly invitationCode: string;
  readonly onInitiate: (candidate: RemoteControlPairingCandidate) => void;
  readonly onInvitationCode: (value: string) => void;
  readonly onSelectCandidate: (candidateId: string) => void;
  readonly palette: ReturnType<typeof useAppPalette>;
  readonly pending: "approve" | "initiate" | "reject" | null;
  readonly selectedCandidateId: string | undefined;
}) {
  if (candidates.length === 0 && selectedCandidateId === undefined) {
    return (
      <Card>
        <Subheading>Looking for nearby nodes</Subheading>
        <Badge>Searching</Badge>
        <BodyText>Open the pairing screen on the node you want to add.</BodyText>
        {commandFeedback}
      </Card>
    );
  }

  const selectedCandidate = candidates.find(
    (candidate) => candidate.candidateId === selectedCandidateId,
  );
  return (
    <Card>
      <Subheading>{candidates.length === 1 ? "Node found" : "Nearby nodes"}</Subheading>
      <Badge
        tone={
          selectedCandidateId !== undefined && selectedCandidate === undefined
            ? "warning"
            : "neutral"
        }
      >
        {selectedCandidateId !== undefined && selectedCandidate === undefined
          ? "Node no longer available"
          : selectedCandidate === undefined
            ? "Choose a node"
            : "Ready to pair"}
      </Badge>
      {candidates.length > 1 ? (
        <BodyText>Choose the node showing the invitation you want to enter.</BodyText>
      ) : null}
      <CardStack>
        {candidates.map((candidate) => {
          const selected = candidate.candidateId === selectedCandidateId;
          const name = candidate.displayName ?? "Nearby node";
          return (
            <Card
              key={candidate.candidateId}
              style={selected ? { borderColor: palette.focus } : undefined}
            >
              <Subheading>{name}</Subheading>
              <KeyValue label="Node ID" value={shortCandidateId(candidate.candidateId)} />
              <BodyText muted>{formatExpiry(candidate.expiresInMillis)}</BodyText>
              <Button
                accessibilityLabel={`${selected ? "Selected" : "Select"} ${name} ${shortCandidateId(candidate.candidateId)}`}
                disabled={pending !== null}
                onPress={() => onSelectCandidate(candidate.candidateId)}
                tone={selected ? "primary" : "secondary"}
              >
                {selected ? "Selected" : "Select this node"}
              </Button>
            </Card>
          );
        })}
      </CardStack>
      {selectedCandidateId !== undefined && selectedCandidate === undefined ? (
        <BodyText>Choose another nearby node, or reopen pairing on this node.</BodyText>
      ) : null}
      {selectedCandidate !== undefined ? (
        <>
          <BodyText>Enter the 8-character invitation shown on the selected node.</BodyText>
          <TextInput
            accessibilityLabel="Invitation code"
            autoCapitalize="characters"
            autoCorrect={false}
            editable={pending === null}
            maxLength={8}
            onChangeText={onInvitationCode}
            placeholder="A1B2C3D4"
            placeholderTextColor={palette.textMuted}
            style={[
              styles.invitation,
              {
                backgroundColor: palette.surface,
                borderColor: palette.border,
                color: palette.text,
              },
            ]}
            value={invitationCode}
          />
          <Button
            disabled={pending !== null || !/^[0-9A-F]{8}$/u.test(invitationCode)}
            onPress={() => onInitiate(selectedCandidate)}
          >
            {pending === "initiate" ? "Submitting…" : "Submit invitation"}
          </Button>
        </>
      ) : null}
      {commandFeedback}
    </Card>
  );
}

function shortCandidateId(candidateId: string): string {
  return candidateId.slice(0, 8).toUpperCase();
}

function isTerminalPairingType(type: RemoteControlPairingState["type"] | undefined): boolean {
  return (
    type === "paired" ||
    type === "rejected" ||
    type === "expired" ||
    type === "cancelled" ||
    type === "failed"
  );
}

function formatExpiry(expiresInMillis: bigint): string {
  const seconds = Number((expiresInMillis + 999n) / 1000n);
  if (seconds <= 1) {
    return "Invitation expires very soon";
  }
  if (seconds < 60) {
    return `${seconds} seconds remaining`;
  }
  const minutes = Math.ceil(seconds / 60);
  return `${minutes} ${minutes === 1 ? "minute" : "minutes"} remaining`;
}

function TerminalPairingCard({
  detail,
  label,
}: {
  readonly detail: string;
  readonly label: string;
}) {
  return (
    <Card>
      <Subheading>{label}</Subheading>
      <Badge tone="warning">Not paired</Badge>
      <BodyText>{detail}</BodyText>
    </Card>
  );
}

function pairingCommandFailure(
  result: RuntimeCommandResult<RemoteControlPairingCommandOutcome>,
): string | null {
  if (result.type === "operationFailure") {
    return "Pairing could not continue. Try again.";
  }
  switch (result.outcome.type) {
    case "accepted":
      return null;
    case "busy":
      return "Another node operation is in progress. Try again shortly.";
    case "failed":
      return pairingFailureMessage(result.outcome.stage);
  }
}

type PairingFailureStage = Extract<
  RemoteControlPairingCommandOutcome,
  { readonly type: "failed" }
>["stage"];

function pairingFailureMessage(stage: PairingFailureStage): string {
  switch (stage) {
    case "input":
      return "Check the invitation and try again.";
    case "candidate":
    case "expired":
      return "The node is no longer available. Reopen pairing on the node and try again.";
    case "route":
      return "No connection path to the node is available yet. Keep its pairing screen open and try again.";
    case "link":
      return "A secure connection to the node could not be opened. Make sure it is on and nearby, then try again.";
    case "identification":
      return "The node could not verify this device. Reopen pairing on the node and try again.";
    case "timeout":
      return "The node did not respond in time. Reopen pairing and try again.";
    case "request":
      return "The node could not complete the pairing request. Keep both devices nearby and try again.";
    case "confirmation":
      return "Confirmation could not be completed. Check both devices and try again.";
    case "persistence":
      return "Pairing could not be saved. Try again.";
    case "node":
      return "This device went offline during pairing. Wait a moment and try again.";
  }
}

const styles = StyleSheet.create({
  invitation: {
    borderRadius: radius.sm,
    borderWidth: 1,
    fontSize: 22,
    fontWeight: "700",
    letterSpacing: 3,
    minHeight: 52,
    paddingHorizontal: space.md,
    paddingVertical: space.sm,
    textAlign: "center",
  },
});
