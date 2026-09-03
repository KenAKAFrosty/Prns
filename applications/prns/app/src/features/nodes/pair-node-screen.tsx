import type {
  DevelopmentNodeSnapshot,
  RemoteControlPairingCommandOutcome,
} from "@prns-internal/expo";
import { useEffect, useState } from "react";
import { StyleSheet, TextInput } from "react-native";

import { useDevelopmentRuntime } from "@/native/development-runtime-context";
import type { RuntimeCommandResult } from "@/native/development-runtime-context";
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
import { formatRequestKind } from "./format";

type RemoteControlPairingState = DevelopmentNodeSnapshot["pairing"];

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
  const pairing = runtime.snapshot?.pairing;

  useEffect(() => {
    if (pairing?.type !== "candidateObserved") {
      setInvitationCode("");
    }
  }, [pairing?.type]);

  const initiate = async (
    state: Extract<RemoteControlPairingState, { type: "candidateObserved" }>,
  ) => {
    setPending("initiate");
    setCommandFailure(null);
    const result = await runtime.initiatePairing({
      candidateId: state.candidate.candidateId,
      invitationCode,
    });
    setCommandFailure(pairingCommandFailure(result));
    setPending(null);
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
        Open the pairing screen on the node you want to add. Enter the invitation shown there, then
        compare the confirmation code on both devices.
      </BodyText>

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
        </Card>
      ) : null}

      {runtime.snapshot === null ? null : (
        <PairingStateCard
          commandFailure={commandFailure}
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
          onInitiate={(state) => void initiate(state)}
          onReject={(state) => void decide("reject", state)}
          palette={palette}
          pairing={runtime.snapshot.pairing}
          pending={pending}
          selectedCandidateId={selectedCandidateId}
        />
      )}

      <NavigationLink href="/nodes">Back to Nodes</NavigationLink>
    </Screen>
  );
}

function PairingStateCard({
  commandFailure,
  invitationCode,
  onApprove,
  onInitiate,
  onInvitationCode,
  onReject,
  palette,
  pairing,
  pending,
  selectedCandidateId,
}: {
  readonly commandFailure: string | null;
  readonly invitationCode: string;
  readonly onApprove: (
    pairing: Extract<RemoteControlPairingState, { type: "confirmationRequired" }>,
  ) => void;
  readonly onInitiate: (
    pairing: Extract<RemoteControlPairingState, { type: "candidateObserved" }>,
  ) => void;
  readonly onInvitationCode: (value: string) => void;
  readonly onReject: (
    pairing: Extract<RemoteControlPairingState, { type: "confirmationRequired" }>,
  ) => void;
  readonly palette: ReturnType<typeof useAppPalette>;
  readonly pairing: RemoteControlPairingState;
  readonly pending: "approve" | "initiate" | "reject" | null;
  readonly selectedCandidateId: string | undefined;
}) {
  const feedback = commandFailure === null ? null : <BodyText>{commandFailure}</BodyText>;

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
        <Card>
          <Subheading>Looking for nearby nodes</Subheading>
          <Badge>Searching</Badge>
          <BodyText>Open the pairing screen on the node you want to add.</BodyText>
          {feedback}
        </Card>
      );
    case "candidateObserved": {
      const selectedCandidateIsCurrent =
        selectedCandidateId === undefined || selectedCandidateId === pairing.candidate.candidateId;
      return (
        <Card>
          <Subheading>Node found</Subheading>
          <Badge tone={selectedCandidateIsCurrent ? "neutral" : "warning"}>
            {selectedCandidateIsCurrent ? "Ready to pair" : "Node no longer available"}
          </Badge>
          {selectedCandidateIsCurrent ? (
            <>
              <BodyText>Enter the 8-character invitation shown on the node.</BodyText>
              <TextInput
                accessibilityLabel="Invitation code"
                autoCapitalize="characters"
                autoCorrect={false}
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
                onPress={() => onInitiate(pairing)}
              >
                {pending === "initiate" ? "Submitting…" : "Submit invitation"}
              </Button>
            </>
          ) : (
            <BodyText>This node is no longer available. Go back and try pairing again.</BodyText>
          )}
          {feedback}
        </Card>
      );
    }
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
        <Card>
          <Subheading>Paired</Subheading>
          <Badge>Ready</Badge>
          <BodyText>This node is now available in Nodes.</BodyText>
          {feedback}
        </Card>
      );
    case "rejected":
      return (
        <TerminalPairingCard
          detail="Pairing was declined on one of the devices."
          label="Rejected"
        />
      );
    case "expired":
      return (
        <TerminalPairingCard
          detail="The invitation expired. Reopen pairing on the node and try again."
          label="Expired"
        />
      );
    case "cancelled":
      return <TerminalPairingCard detail="Pairing was cancelled." label="Cancelled" />;
    case "failed":
      return (
        <TerminalPairingCard detail={pairingFailureMessage(pairing.stage)} label="Pairing failed" />
      );
  }
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
    case "link":
    case "identification":
    case "request":
      return "The node could not be reached. Keep its pairing screen open and try again.";
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
