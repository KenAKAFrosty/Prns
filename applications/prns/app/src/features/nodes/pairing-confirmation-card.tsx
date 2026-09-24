import * as Bindings from "@prns-internal/expo";
import type { DevelopmentNodeSnapshot } from "@prns-internal/expo";
import { useState } from "react";
import { StyleSheet, Text } from "react-native";

import { Badge, BodyText, Button, Card, CardStack, KeyValue, Subheading } from "@/ui/primitives";
import { useAppPalette } from "@/ui/theme";
import { formatBytes, formatControllerAuthority, formatRequestKind } from "./format";
import { summarizePairingAccess } from "./pairing-access-summary";

type Confirmation = Extract<DevelopmentNodeSnapshot["pairing"], { tag: "ConfirmationRequired" }>;

export function PairingConfirmationCard({
  pairing,
  pending,
  commandFailure,
  onApprove,
  onReject,
}: {
  readonly pairing: Confirmation;
  readonly pending: "approve" | "initiate" | "reject" | null;
  readonly commandFailure: string | null;
  readonly onApprove: (pairing: Confirmation) => void;
  readonly onReject: (pairing: Confirmation) => void;
}) {
  const palette = useAppPalette();
  const [detailsVisible, setDetailsVisible] = useState(false);
  const { authority, confirmationCode, permissions, targetIdentityFingerprint } = pairing.inner;

  return (
    <Card>
      <Subheading>Confirmation required</Subheading>
      <BodyText>Compare this code on both devices.</BodyText>
      <Text
        accessibilityLabel={`Confirmation code ${confirmationCode.split("").join(" ")}`}
        selectable
        style={[styles.code, { color: palette.text }]}
      >
        {confirmationCode}
      </Text>
      <Badge>{formatControllerAuthority(authority)}</Badge>
      <BodyText muted>Requested access</BodyText>
      <BodyText>{summarizePairingAccess(permissions)}</BodyText>
      {authority === Bindings.RemoteControlControllerAuthority.Administrator ? (
        <BodyText>This device can also grant or remove access for other devices.</BodyText>
      ) : null}
      <Button
        accessibilityState={{ expanded: detailsVisible }}
        onPress={() => setDetailsVisible((visible) => !visible)}
        tone="secondary"
      >
        {detailsVisible ? "Hide pairing details" : "Show pairing details"}
      </Button>
      {detailsVisible ? (
        <CardStack>
          <KeyValue label="Node ID" value={formatBytes(targetIdentityFingerprint)} />
          <Subheading>Requested controls</Subheading>
          {permissions.length === 0 ? (
            <BodyText>No node controls requested.</BodyText>
          ) : (
            permissions.map((permission) => (
              <BodyText key={permission}>{formatRequestKind(permission)}</BodyText>
            ))
          )}
        </CardStack>
      ) : null}
      <BodyText>If the codes match, approve on the node first, then approve here.</BodyText>
      <CardStack>
        <Button disabled={pending !== null} onPress={() => onApprove(pairing)}>
          {pending === "approve" ? "Approving…" : "Codes match — approve"}
        </Button>
        <Button disabled={pending !== null} onPress={() => onReject(pairing)} tone="destructive">
          {pending === "reject" ? "Rejecting…" : "Reject pairing"}
        </Button>
      </CardStack>
      {commandFailure === null ? null : <BodyText>{commandFailure}</BodyText>}
    </Card>
  );
}

const styles = StyleSheet.create({
  code: {
    fontSize: 36,
    fontWeight: "700",
    fontVariant: ["tabular-nums"],
    letterSpacing: 5,
    textAlign: "center",
    paddingVertical: 8,
  },
});
