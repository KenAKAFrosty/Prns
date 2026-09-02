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
import { formatBluetooth, formatBytes, formatRequestKind } from "./format";

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
      <Badge>Upstream RemoteControl pairing</Badge>
      <ScreenHeading>Pair a node</ScreenHeading>
      <BodyText>
        Open Pair Remote Control on the E290, then use only the invitation and confirmation values
        shown by the two devices.
      </BodyText>

      {runtime.phase === "unavailable" ? (
        <Card>
          <Subheading>Bluetooth unavailable</Subheading>
          <Badge tone="warning">No {runtime.availability.platform} native provider</Badge>
          <BodyText>
            Pairing cannot run on this platform, and this screen does not manufacture candidates or
            authorization state.
          </BodyText>
        </Card>
      ) : null}

      {runtime.phase === "starting" ? (
        <Card>
          <Subheading>Starting the local node</Subheading>
          <Badge>Starting</Badge>
          <BodyText muted>
            Waiting for persistence restore before pairing commands are admitted.
          </BodyText>
        </Card>
      ) : null}

      {runtime.phase === "failed" ? (
        <Card>
          <Subheading>Local node failed to start</Subheading>
          <Badge tone="warning">Pairing unavailable</Badge>
          <BodyText>
            {runtime.lifecycleFailure ?? "The native provider did not return a reason."}
          </BodyText>
        </Card>
      ) : null}

      {runtime.snapshot === null ? null : (
        <>
          <Card>
            <Subheading>Transport readiness</Subheading>
            <KeyValue label="Bluetooth Auto" value={formatBluetooth(runtime.snapshot.bluetooth)} />
            <KeyValue label="Snapshot revision" value={runtime.snapshot.revision.toString()} />
          </Card>
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
        </>
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
          <BodyText>Bluetooth Auto is not ready, so no pairing candidate is available.</BodyText>
          {feedback}
        </Card>
      );
    case "searching":
      return (
        <Card>
          <Subheading>Searching</Subheading>
          <Badge>Waiting for signed availability</Badge>
          <BodyText>
            Keep the E290 pairing window open. Only a currently observed, unexpired candidate can
            accept an invitation.
          </BodyText>
          {feedback}
        </Card>
      );
    case "candidateObserved": {
      const selectedCandidateIsCurrent =
        selectedCandidateId === undefined || selectedCandidateId === pairing.candidate.candidateId;
      return (
        <Card>
          <Subheading>Candidate observed</Subheading>
          <Badge tone={selectedCandidateIsCurrent ? "neutral" : "warning"}>
            {selectedCandidateIsCurrent ? "Signed availability" : "Requested candidate is stale"}
          </Badge>
          <KeyValue label="Candidate ID" value={pairing.candidate.candidateId} />
          <KeyValue label="Endpoint" value={formatBytes(pairing.candidate.endpoint)} />
          <KeyValue label="Observed (ms)" value={pairing.candidate.observedAtMillis.toString()} />
          <KeyValue label="Expires (ms)" value={pairing.candidate.expiresAtMillis.toString()} />
          <KeyValue
            label="Public app data"
            value={
              pairing.candidate.publicAppData.length === 0
                ? "Empty"
                : formatBytes(pairing.candidate.publicAppData)
            }
          />
          {selectedCandidateIsCurrent ? (
            <>
              <BodyText>
                Enter the exact eight-character hexadecimal invitation from the E290.
              </BodyText>
              <TextInput
                accessibilityLabel="E290 invitation code"
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
            <BodyText>
              The candidate named by this route is no longer current. Return to Nodes and reopen
              pairing before submitting any invitation.
            </BodyText>
          )}
          {feedback}
        </Card>
      );
    }
    case "invitationSubmitted":
      return (
        <Card>
          <Subheading>Invitation submitted</Subheading>
          <Badge>Not paired yet</Badge>
          <KeyValue label="Candidate ID" value={pairing.candidateId} />
          <BodyText>Waiting for the upstream pairing exchange to produce a confirmation.</BodyText>
          {feedback}
        </Card>
      );
    case "confirmationRequired":
      return (
        <Card>
          <Subheading>Confirmation required</Subheading>
          <Badge tone="warning">Compare both devices</Badge>
          <KeyValue label="Confirmation code" value={pairing.confirmationCode} />
          <KeyValue label="Attempt ID" value={pairing.attemptId} />
          <KeyValue
            label="Target fingerprint"
            value={formatBytes(pairing.targetIdentityFingerprint)}
          />
          <KeyValue
            label="Requested permissions"
            value={
              pairing.permissions.length === 0
                ? "None"
                : pairing.permissions.map(formatRequestKind).join(", ")
            }
          />
          <BodyText>
            Approve only if this confirmation code exactly matches the code on the E290. Approval
            here is not success; the E290 must also approve and both records must persist.
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
          <Subheading>Awaiting target approval</Subheading>
          <Badge>Not paired yet</Badge>
          <KeyValue label="Attempt ID" value={pairing.attemptId} />
          <BodyText>
            Your local approval was accepted. Approve the same attempt on the E290.
          </BodyText>
          {feedback}
        </Card>
      );
    case "persisting":
      return (
        <Card>
          <Subheading>Persisting on both devices</Subheading>
          <Badge>Not paired yet</Badge>
          <KeyValue label="Attempt ID" value={pairing.attemptId} />
          <BodyText>
            Both approvals have completed, but durable authorization has not yet been confirmed.
          </BodyText>
          {feedback}
        </Card>
      );
    case "paired":
      return (
        <Card>
          <Subheading>Paired</Subheading>
          <Badge>Authorization persisted</Badge>
          <KeyValue label="Attempt ID" value={pairing.attemptId} />
          <BodyText>
            The controller-side persisted-target inventory now owns this relationship.
          </BodyText>
          {feedback}
        </Card>
      );
    case "rejected":
      return <TerminalPairingCard detail={pairing.detail} label="Rejected" />;
    case "expired":
      return <TerminalPairingCard detail={pairing.detail} label="Expired" />;
    case "cancelled":
      return (
        <TerminalPairingCard detail="The active pairing attempt was cancelled." label="Cancelled" />
      );
    case "failed":
      return (
        <TerminalPairingCard
          detail={`${pairing.stage}: ${pairing.detail}`}
          label="Pairing failed"
        />
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
    return result.detail;
  }
  switch (result.outcome.type) {
    case "accepted":
      return null;
    case "busy":
      return "Another native pairing or target operation is already active.";
    case "failed":
      return `${result.outcome.stage}: ${result.outcome.detail}`;
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
