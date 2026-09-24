import * as Bindings from "@prns-internal/expo";
import type {
  DevelopmentNodeSnapshot,
  RemoteControlPairingCommandOutcome,
} from "@prns-internal/expo";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { StyleSheet } from "react-native";

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
import { TextField } from "@/ui/text-field";
import { useAppPalette } from "@/ui/theme";
import { AndroidBluetoothCard } from "./android-bluetooth-card";
import { IosBluetoothCard } from "./ios-bluetooth-card";
import { PairingConfirmationCard } from "./pairing-confirmation-card";

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
  const [commandFailure, setCommandFailure] = useState<string | null>(null);
  const [candidateId, setCandidateId] = useState<string | undefined>(selectedCandidateId);
  const pairing = runtime.snapshot?.pairing;
  const confirming = pairing?.tag === Bindings.RemoteControlPairingState_Tags.ConfirmationRequired;
  const candidates = runtime.snapshot?.pairingCandidates ?? [];
  const selectedCandidateIsPresent = candidates.some(
    (candidate) => candidate.candidateId === candidateId,
  );
  const candidateSelectionKey = `${candidateId ?? "none"}:${selectedCandidateIsPresent ? "present" : "missing"}`;
  const previousCandidateSelectionKey = useRef(candidateSelectionKey);

  useEffect(() => {
    if (pairing?.tag !== Bindings.RemoteControlPairingState_Tags.Searching) {
      setInvitationCode("");
    }
  }, [pairing?.tag]);

  useEffect(() => {
    setCandidateId(selectedCandidateId);
  }, [selectedCandidateId]);

  useEffect(() => {
    setCandidateId((current) =>
      current === undefined && candidates.length === 1 ? candidates[0]?.candidateId : current,
    );
  }, [candidates]);

  useEffect(() => {
    if (!isTerminalPairingType(pairing?.tag)) {
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
  }, [candidates, pairing?.tag]);

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

  const decide = async (
    decision: "approve" | "reject",
    state: Extract<RemoteControlPairingState, { tag: "ConfirmationRequired" }>,
  ) => {
    setPending(decision);
    setCommandFailure(null);
    const result = await (decision === "approve" ? runtime.approvePairing : runtime.rejectPairing)({
      attemptId: state.inner.attemptId,
    });
    setCommandFailure(pairingCommandFailure(result));
    setPending(null);
  };

  return (
    <Screen>
      <Badge>Secure pairing</Badge>
      <ScreenHeading>Pair a node</ScreenHeading>
      {confirming ? null : (
        <BodyText>
          Open secure pairing on the node you want to manage. When it appears below, enter its
          invitation and compare the confirmation code on both devices.
        </BodyText>
      )}

      {confirming ? null : <AndroidBluetoothCard />}
      {confirming ? null : <IosBluetoothCard />}

      {runtime.phase === "unavailable" ? (
        <Card>
          <Subheading>Pairing unavailable</Subheading>
          <Badge tone="warning">Not supported on {runtime.availability.platform}</Badge>
          <BodyText>Node pairing is not available on this platform yet.</BodyText>
        </Card>
      ) : null}

      {runtime.phase === "starting" ? (
        <Card>
          <Subheading>Getting ready</Subheading>
          <Badge>Starting</Badge>
          <BodyText muted>Pairing will be available in a moment.</BodyText>
        </Card>
      ) : null}

      {runtime.phase === "failed" ? (
        <Card>
          <Subheading>This device&apos;s node failed to start</Subheading>
          <Badge tone="warning">Pairing unavailable</Badge>
          <BodyText>
            Pairing cannot start because this device&apos;s node is unavailable. Open its
            diagnostics for more details.
          </BodyText>
          <NavigationLink href="/nodes/local">View diagnostics</NavigationLink>
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

      <NavigationLink href="/nodes" direction="back">
        Back to Nodes
      </NavigationLink>
    </Screen>
  );
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
    pairing: Extract<RemoteControlPairingState, { tag: "ConfirmationRequired" }>,
  ) => void;
  readonly onInitiate: (candidate: RemoteControlPairingCandidate) => void;
  readonly onInvitationCode: (value: string) => void;
  readonly onReject: (
    pairing: Extract<RemoteControlPairingState, { tag: "ConfirmationRequired" }>,
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

  switch (pairing.tag) {
    case Bindings.RemoteControlPairingState_Tags.BluetoothUnavailable:
      return (
        <Card>
          <Subheading>Bluetooth unavailable</Subheading>
          <Badge tone="warning">Cannot search</Badge>
          <BodyText>Turn on Bluetooth and make sure prns has permission to use it.</BodyText>
          {feedback}
        </Card>
      );
    case Bindings.RemoteControlPairingState_Tags.Searching:
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
    case Bindings.RemoteControlPairingState_Tags.InvitationSubmitted:
      return (
        <Card>
          <Subheading>Invitation sent</Subheading>
          <Badge>Waiting for confirmation</Badge>
          <BodyText>Check both devices for a confirmation code.</BodyText>
          {feedback}
        </Card>
      );
    case Bindings.RemoteControlPairingState_Tags.ConfirmationRequired:
      return (
        <PairingConfirmationCard
          key={pairing.inner.attemptId}
          commandFailure={commandFailure}
          onApprove={onApprove}
          onReject={onReject}
          pairing={pairing}
          pending={pending}
        />
      );
    case Bindings.RemoteControlPairingState_Tags.AwaitingTargetApproval:
      return (
        <Card>
          <Subheading>Waiting for the node</Subheading>
          <Badge>Approve on the node</Badge>
          <BodyText>Approve pairing on the node to continue.</BodyText>
          {feedback}
        </Card>
      );
    case Bindings.RemoteControlPairingState_Tags.Persisting:
      return (
        <Card>
          <Subheading>Finishing pairing</Subheading>
          <Badge>Saving</Badge>
          <BodyText>Saving this connection on both devices.</BodyText>
          {feedback}
        </Card>
      );
    case Bindings.RemoteControlPairingState_Tags.Paired:
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
    case Bindings.RemoteControlPairingState_Tags.Rejected:
      return (
        <>
          <TerminalPairingCard
            detail="Pairing was declined on one of the devices."
            label="Rejected"
          />
          {remainingCandidateChooser}
        </>
      );
    case Bindings.RemoteControlPairingState_Tags.Expired:
      return (
        <>
          <TerminalPairingCard
            detail="The invitation expired. Reopen pairing on the node and try again."
            label="Expired"
          />
          {remainingCandidateChooser}
        </>
      );
    case Bindings.RemoteControlPairingState_Tags.Cancelled:
      return (
        <>
          <TerminalPairingCard detail="Pairing was cancelled." label="Cancelled" />
          {remainingCandidateChooser}
        </>
      );
    case Bindings.RemoteControlPairingState_Tags.Failed:
      return (
        <>
          <TerminalPairingCard
            detail={pairingFailureMessage(pairing.inner.stage)}
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
                accessibilityState={{ selected }}
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
          <TextField
            label="Invitation code"
            autoCapitalize="characters"
            autoCorrect={false}
            editable={pending === null}
            maxLength={8}
            onChangeText={onInvitationCode}
            placeholder="A1B2C3D4"
            style={styles.invitation}
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

function isTerminalPairingType(type: RemoteControlPairingState["tag"] | undefined): boolean {
  return (
    type === Bindings.RemoteControlPairingState_Tags.Paired ||
    type === Bindings.RemoteControlPairingState_Tags.Rejected ||
    type === Bindings.RemoteControlPairingState_Tags.Expired ||
    type === Bindings.RemoteControlPairingState_Tags.Cancelled ||
    type === Bindings.RemoteControlPairingState_Tags.Failed
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
  switch (result.outcome.tag) {
    case Bindings.RemoteControlPairingCommandOutcome_Tags.Accepted:
      return null;
    case Bindings.RemoteControlPairingCommandOutcome_Tags.Busy:
      return "Another node operation is in progress. Try again shortly.";
    case Bindings.RemoteControlPairingCommandOutcome_Tags.Failed:
      return pairingFailureMessage(result.outcome.inner.stage);
  }
}

type PairingFailureStage = Extract<
  RemoteControlPairingCommandOutcome,
  { readonly tag: "Failed" }
>["inner"]["stage"];

function pairingFailureMessage(stage: PairingFailureStage): string {
  switch (stage) {
    case Bindings.RemoteControlPairingFailureStage.Input:
      return "Check the invitation and try again.";
    case Bindings.RemoteControlPairingFailureStage.Candidate:
    case Bindings.RemoteControlPairingFailureStage.Expired:
      return "The node is no longer available. Reopen pairing on the node and try again.";
    case Bindings.RemoteControlPairingFailureStage.Route:
      return "No connection path to the node is available yet. Keep its pairing screen open and try again.";
    case Bindings.RemoteControlPairingFailureStage.Link:
      return "A secure connection to the node could not be opened. Make sure it is on and nearby, then try again.";
    case Bindings.RemoteControlPairingFailureStage.Identification:
      return "The node could not verify this device. Reopen pairing on the node and try again.";
    case Bindings.RemoteControlPairingFailureStage.Timeout:
      return "The node did not respond in time. Reopen pairing and try again.";
    case Bindings.RemoteControlPairingFailureStage.Request:
      return "The node could not complete the pairing request. Keep both devices nearby and try again.";
    case Bindings.RemoteControlPairingFailureStage.Confirmation:
      return "Confirmation could not be completed. Check both devices and try again.";
    case Bindings.RemoteControlPairingFailureStage.Persistence:
      return "Pairing could not be saved. Try again.";
    case Bindings.RemoteControlPairingFailureStage.Node:
      return "This device went offline during pairing. Wait a moment and try again.";
  }
}

const styles = StyleSheet.create({
  invitation: {
    fontSize: 22,
    fontWeight: "700",
    letterSpacing: 3,
    minHeight: 52,
    textAlign: "center",
  },
});
