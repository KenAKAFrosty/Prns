import type {
  AnnounceLxmfOutcome,
  CancelLxmfMessageOutcome,
  Contact,
  ContactListOutcome,
  LxmfMessage,
  LxmfMessageListOutcome,
  LxmfPeerListOutcome,
  LxmfPeerSummary,
  MeasureLxmfTextOutcome,
  RetryLxmfMessageOutcome,
  SendDirectTextOutcome,
} from "@prns-internal/expo";
import type { Href } from "expo-router";
import { useRouter } from "expo-router";
import type { DestinationHash } from "personal-rns/contract";
import { useCallback, useEffect, useMemo, useState } from "react";
import { StyleSheet, TextInput, View } from "react-native";

import { formatContactHash, parseDestinationHash } from "@/features/contacts/format";
import { useContactRuntime } from "@/native/contact-runtime-context";
import {
  type RuntimeCommandResult,
  useDevelopmentRuntime,
} from "@/native/development-runtime-context";
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
import { radius, space, useAppPalette } from "@/ui/theme";
import {
  deliveryLabel,
  messagePeer,
  peerLabel,
  textPresentation,
  timestampLabel,
  verificationLabel,
} from "./format";

const pageLimit = 100;

type LxmfData = {
  readonly peers: readonly LxmfPeerSummary[];
  readonly messages: readonly LxmfMessage[];
  readonly contacts: readonly Contact[];
  readonly pending: boolean;
  readonly failure: string | null;
  readonly refresh: () => Promise<void>;
};

function useLxmfData(peer: DestinationHash | null): LxmfData {
  const development = useDevelopmentRuntime();
  const contactRuntime = useContactRuntime();
  const [peers, setPeers] = useState<readonly LxmfPeerSummary[]>([]);
  const [messages, setMessages] = useState<readonly LxmfMessage[]>([]);
  const [contacts, setContacts] = useState<readonly Contact[]>([]);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const peerKey = peer === null ? null : formatContactHash(peer);
  const nodeRunning = development.phase === "ready" && development.snapshot?.runtime === "running";

  const refresh = useCallback(async () => {
    if (development.availability.type !== "available" || development.phase === "starting") {
      return;
    }
    const selectedPeer = peerKey === null ? null : parseDestinationHash(peerKey);
    if (peerKey !== null && selectedPeer === null) {
      setFailure("The selected conversation destination is invalid.");
      return;
    }
    setPending(true);
    setFailure(null);
    const [peerResult, messageResult] = await Promise.all([
      nodeRunning ? development.listLxmfPeers() : Promise.resolve(null),
      development.listLxmfMessages({ peer: selectedPeer, before: null, limit: pageLimit }),
    ]);
    if (peerResult === null) {
      setPeers([]);
    } else {
      applyPeerResult(peerResult, setPeers, setFailure);
    }
    applyMessageResult(messageResult, setMessages, setFailure);
    if (contactRuntime.runtime !== null) {
      try {
        applyContactResult(await contactRuntime.runtime.listContacts(), setContacts);
      } catch (error) {
        setFailure(formatThrown(error));
      }
    }
    setPending(false);
  }, [
    contactRuntime.runtime,
    development.availability.type,
    development.listLxmfMessages,
    development.listLxmfPeers,
    development.phase,
    nodeRunning,
    peerKey,
  ]);

  useEffect(() => {
    if (development.phase === "starting" && development.snapshot?.revision === undefined) {
      return;
    }
    void refresh();
  }, [development.phase, development.snapshot?.revision, refresh]);

  return { peers, messages, contacts, pending, failure, refresh };
}

export function InboxScreen() {
  const development = useDevelopmentRuntime();
  const data = useLxmfData(null);
  const [command, setCommand] = useState<string | null>(null);
  const [announcing, setAnnouncing] = useState(false);
  const nodeRunning = development.phase === "ready" && development.snapshot?.runtime === "running";

  const announce = async () => {
    setAnnouncing(true);
    setCommand(null);
    const result = await development.announceLxmf();
    setCommand(
      result.type === "operationFailure" ? result.detail : announceOutcomeLabel(result.outcome),
    );
    setAnnouncing(false);
    await development.refreshSnapshot();
  };

  const conversations = useMemo(
    () => conversationDestinations(data.peers, data.messages),
    [data.messages, data.peers],
  );

  return (
    <Screen>
      <Badge>Durable direct LXMF</Badge>
      <ScreenHeading>Inbox</ScreenHeading>
      <BodyText>
        Messages are stored in the application mailbox. Listing, exact-wire retry, and cancellation
        remain available while the local node is stopped; sending and peer discovery require a
        running generation.
      </BodyText>
      <LxmfHealthCard />
      {development.availability.type !== "available" ? (
        <UnavailableCard platform={development.availability.platform} />
      ) : (
        <>
          {nodeRunning ? null : (
            <Card>
              <Badge tone="warning">
                {development.phase === "starting" ? "Starting" : "Mailbox offline"}
              </Badge>
              <BodyText>
                {development.lifecycleFailure ??
                  (development.phase === "starting"
                    ? "Waiting for persistence restoration before network attempts resume."
                    : "The local node is stopped. Durable messages remain available.")}
              </BodyText>
            </Card>
          )}
          <View style={styles.actions}>
            {nodeRunning ? (
              <Button disabled={announcing} onPress={() => void announce()}>
                {announcing ? "Announcing…" : "Announce LXMF destination"}
              </Button>
            ) : null}
            <Button disabled={data.pending} onPress={() => void data.refresh()} tone="secondary">
              {data.pending ? "Refreshing…" : "Refresh Inbox"}
            </Button>
          </View>
          {command === null ? null : (
            <Card>
              <BodyText>{command}</BodyText>
            </Card>
          )}
          {data.failure === null ? null : <FailureCard detail={data.failure} />}
          {conversations.length === 0 ? (
            <Card>
              <Badge>No conversations</Badge>
              <BodyText>
                {nodeRunning
                  ? "Wait for a compatible lxmf.delivery announce from the controlled peer, then open the composer."
                  : "No durable messages are stored in this development mailbox."}
              </BodyText>
            </Card>
          ) : (
            conversations.map((destination) => {
              const encoded = formatContactHash(destination);
              const compatiblePeer = data.peers.find(
                (peer) => formatContactHash(peer.destination) === encoded,
              );
              const peerMessages = data.messages.filter(
                (message) => formatContactHash(messagePeer(message)) === encoded,
              );
              const latest = peerMessages[0];
              const href: Href = {
                pathname: "/inbox/conversation/[destination]",
                params: { destination: encoded },
              };
              return (
                <Card key={encoded}>
                  <Subheading>{peerLabel(destination, data.peers, data.contacts)}</Subheading>
                  <KeyValue label="Destination" value={encoded} />
                  <KeyValue
                    label="Last observed"
                    value={
                      compatiblePeer === undefined
                        ? "No compatible announce retained"
                        : `${compatiblePeer.lastObservedAgeMillis.toString()} ms ago`
                    }
                  />
                  <KeyValue label="Messages" value={peerMessages.length.toString()} />
                  {latest === undefined ? null : (
                    <BodyText muted>
                      {textPresentation(latest.content).text} · {deliveryLabel(latest)}
                    </BodyText>
                  )}
                  <NavigationLink href={href}>Open conversation</NavigationLink>
                </Card>
              );
            })
          )}
          {nodeRunning ? (
            <NavigationLink href="/inbox/compose">Compose by destination</NavigationLink>
          ) : (
            <BodyText muted>
              Start the local node to discover peers or compose a new message.
            </BodyText>
          )}
        </>
      )}
    </Screen>
  );
}

export function ConversationScreen({ destination }: { readonly destination: DestinationHash }) {
  const development = useDevelopmentRuntime();
  const data = useLxmfData(destination);
  const [mutationStatus, setMutationStatus] = useState<string | null>(null);
  const [activeMutationId, setActiveMutationId] = useState<bigint | null>(null);
  const encoded = formatContactHash(destination);
  const peer = data.peers.find((candidate) => formatContactHash(candidate.destination) === encoded);
  const nodeRunning = development.phase === "ready" && development.snapshot?.runtime === "running";

  const mutate = async (kind: "retry" | "cancel", localRecordId: bigint): Promise<void> => {
    setActiveMutationId(localRecordId);
    setMutationStatus(null);
    const result =
      kind === "retry"
        ? await development.retryLxmfMessage(localRecordId)
        : await development.cancelLxmfMessage(localRecordId);
    setMutationStatus(
      result.type === "operationFailure"
        ? result.detail
        : mailboxMutationLabel(kind, result.outcome),
    );
    await data.refresh();
    setActiveMutationId(null);
  };

  if (development.availability.type !== "available") {
    return (
      <Screen>
        <ScreenHeading>Conversation</ScreenHeading>
        <UnavailableCard platform={development.availability.platform} />
        <NavigationLink href="/inbox">Back to Inbox</NavigationLink>
      </Screen>
    );
  }
  return (
    <Screen>
      <Badge>Durable conversation</Badge>
      <ScreenHeading>{peerLabel(destination, data.peers, data.contacts)}</ScreenHeading>
      <KeyValue label="Destination" value={encoded} />
      {nodeRunning ? null : (
        <Card>
          <Badge tone="warning">
            {development.phase === "starting" ? "Starting" : "Mailbox offline"}
          </Badge>
          <BodyText>
            {development.lifecycleFailure ??
              (development.phase === "starting"
                ? "Waiting for persistence restoration before network attempts resume."
                : "The local node is stopped. You can still inspect, retry, or cancel durable records.")}
          </BodyText>
        </Card>
      )}
      {peer?.requiredStampCost === null || peer?.requiredStampCost === undefined ? null : (
        <Card>
          <Badge tone="warning">Unsupported stamp requirement</Badge>
          <BodyText>
            This peer requires stamp cost {peer.requiredStampCost.toString()}; direct send is
            visible but unavailable in this slice.
          </BodyText>
        </Card>
      )}
      {data.failure === null ? null : <FailureCard detail={data.failure} />}
      {mutationStatus === null ? null : (
        <Card>
          <BodyText>{mutationStatus}</BodyText>
        </Card>
      )}
      {nodeRunning ? (
        <Composer destination={destination} onSettled={data.refresh} />
      ) : (
        <BodyText muted>Start the local node to compose a new message.</BodyText>
      )}
      <Subheading>Messages</Subheading>
      {data.messages.length === 0 ? (
        <Card>
          <BodyText muted>No durable messages are stored for this destination.</BodyText>
        </Card>
      ) : (
        data.messages.map((message) => (
          <MessageCard
            key={message.localRecordId.toString()}
            message={message}
            pending={activeMutationId === message.localRecordId}
            onRetry={
              message.deliveryState.type === "failed"
                ? () => mutate("retry", message.localRecordId)
                : undefined
            }
            onCancel={
              message.deliveryState.type === "queued" || message.deliveryState.type === "sending"
                ? () => mutate("cancel", message.localRecordId)
                : undefined
            }
          />
        ))
      )}
      <NavigationLink href="/inbox">Back to Inbox</NavigationLink>
    </Screen>
  );
}

export function ComposeScreen({
  initialDestination,
}: {
  readonly initialDestination: string | null;
}) {
  const development = useDevelopmentRuntime();
  const router = useRouter();
  const [destinationText, setDestinationText] = useState(initialDestination ?? "");
  const destination = parseDestinationHash(destinationText);
  const palette = useAppPalette();
  const nodeRunning = development.phase === "ready" && development.snapshot?.runtime === "running";

  if (development.availability.type !== "available") {
    return (
      <Screen>
        <ScreenHeading>Compose</ScreenHeading>
        <UnavailableCard platform={development.availability.platform} />
        <NavigationLink href="/inbox">Back to Inbox</NavigationLink>
      </Screen>
    );
  }
  if (!nodeRunning) {
    return (
      <Screen>
        <ScreenHeading>Compose</ScreenHeading>
        <Card>
          <Badge>{development.phase === "starting" ? "Starting" : "Local node stopped"}</Badge>
          <BodyText>
            {development.lifecycleFailure ??
              "Start the local node before composing a new direct message."}
          </BodyText>
        </Card>
        <NavigationLink href="/inbox">Back to Inbox</NavigationLink>
      </Screen>
    );
  }

  return (
    <Screen>
      <Badge>Direct LXMF</Badge>
      <ScreenHeading>Compose</ScreenHeading>
      <BodyText>
        Enter an observed lxmf.delivery destination. Native Rust measures and signs the complete
        wire once, commits it to the durable queue, and reports delivery only after transport proof.
      </BodyText>
      <TextInput
        accessibilityLabel="LXMF destination hash"
        autoCapitalize="none"
        autoCorrect={false}
        onChangeText={setDestinationText}
        placeholder="32 hexadecimal characters"
        placeholderTextColor={palette.textMuted}
        style={[
          styles.input,
          { backgroundColor: palette.surface, borderColor: palette.border, color: palette.text },
        ]}
        value={destinationText}
      />
      {destination === null ? (
        <Card>
          <Badge tone="warning">Destination required</Badge>
          <BodyText>Enter exactly 32 hexadecimal characters.</BodyText>
        </Card>
      ) : (
        <>
          <Composer
            destination={destination}
            onSettled={async (result) => {
              if (result.type !== "outcome" || result.outcome.type !== "accepted") {
                return;
              }
              const href: Href = {
                pathname: "/inbox/conversation/[destination]",
                params: { destination: formatContactHash(destination) },
              };
              router.replace(href);
            }}
          />
          <NavigationLink
            href={{
              pathname: "/inbox/conversation/[destination]",
              params: { destination: formatContactHash(destination) },
            }}
          >
            Open conversation
          </NavigationLink>
        </>
      )}
      <NavigationLink href="/inbox">Back to Inbox</NavigationLink>
    </Screen>
  );
}

function Composer({
  destination,
  onSettled,
}: {
  readonly destination: DestinationHash;
  readonly onSettled: (result: RuntimeCommandResult<SendDirectTextOutcome>) => Promise<void>;
}) {
  const development = useDevelopmentRuntime();
  const palette = useAppPalette();
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [measurement, setMeasurement] = useState<MeasureLxmfTextOutcome | null>(null);
  const [measureFailure, setMeasureFailure] = useState<string | null>(null);
  const [sendOutcome, setSendOutcome] = useState<SendDirectTextOutcome | null>(null);
  const [sendFailure, setSendFailure] = useState<string | null>(null);
  const [sending, setSending] = useState(false);

  useEffect(() => {
    let current = true;
    void development.measureLxmfText({ title, content }).then((result) => {
      if (!current) {
        return;
      }
      if (result.type === "operationFailure") {
        setMeasureFailure(result.detail);
        setMeasurement(null);
      } else {
        setMeasureFailure(null);
        setMeasurement(result.outcome);
      }
    });
    return () => {
      current = false;
    };
  }, [content, development.measureLxmfText, title]);

  const send = async (nextTitle = title, nextContent = content) => {
    setSending(true);
    setSendOutcome(null);
    setSendFailure(null);
    const result = await development.sendDirectText({
      destination,
      title: nextTitle,
      content: nextContent,
    });
    if (result.type === "operationFailure") {
      setSendFailure(result.detail);
    } else {
      setSendOutcome(result.outcome);
      if (result.outcome.type === "accepted") {
        setTitle("");
        setContent("");
      }
    }
    setSending(false);
    await development.refreshSnapshot();
    await onSettled(result);
  };

  return (
    <Card>
      <Subheading>New message</Subheading>
      <TextInput
        accessibilityLabel="LXMF title"
        onChangeText={setTitle}
        placeholder="Title"
        placeholderTextColor={palette.textMuted}
        style={[
          styles.input,
          {
            backgroundColor: palette.surfaceRaised,
            borderColor: palette.border,
            color: palette.text,
          },
        ]}
        value={title}
      />
      <TextInput
        accessibilityLabel="LXMF message"
        multiline
        onChangeText={setContent}
        placeholder="Message"
        placeholderTextColor={palette.textMuted}
        style={[
          styles.input,
          styles.messageInput,
          {
            backgroundColor: palette.surfaceRaised,
            borderColor: palette.border,
            color: palette.text,
          },
        ]}
        value={content}
      />
      <Measurement outcome={measurement} failure={measureFailure} />
      <Button
        disabled={
          sending || content.length === 0 || measurement === null || measurement.type !== "measured"
        }
        onPress={() => void send()}
      >
        {sending ? "Saving to durable queue…" : "Send direct message"}
      </Button>
      {sendFailure === null ? null : <BodyText>Native send unavailable: {sendFailure}</BodyText>}
      <SendResult outcome={sendOutcome} />
    </Card>
  );
}

function MessageCard({
  message,
  onRetry,
  onCancel,
  pending,
}: {
  readonly message: LxmfMessage;
  readonly onRetry: (() => Promise<void>) | undefined;
  readonly onCancel: (() => Promise<void>) | undefined;
  readonly pending: boolean;
}) {
  const title = textPresentation(message.title);
  const content = textPresentation(message.content);
  const unverified = message.verification !== "verified";

  return (
    <Card>
      <Badge tone={unverified || !title.validUtf8 || !content.validUtf8 ? "warning" : "neutral"}>
        {message.direction === "inbound" ? "Inbound" : "Outbound"}
      </Badge>
      <Subheading>{title.text.length === 0 ? "Untitled" : title.text}</Subheading>
      <BodyText>{content.text}</BodyText>
      <KeyValue label="Time" value={timestampLabel(message.timestamp)} />
      <KeyValue label="Verification" value={verificationLabel(message)} />
      <KeyValue label="Delivery" value={deliveryLabel(message)} />
      <KeyValue label="Message ID" value={formatContactHash(message.messageId)} />
      {onRetry === undefined ? null : (
        <Button disabled={pending} onPress={() => void onRetry()} tone="secondary">
          {pending ? "Retrying…" : "Retry exact stored message"}
        </Button>
      )}
      {onCancel === undefined ? null : (
        <Button disabled={pending} onPress={() => void onCancel()} tone="secondary">
          {pending ? "Cancelling…" : "Cancel queued message"}
        </Button>
      )}
    </Card>
  );
}

function Measurement({
  outcome,
  failure,
}: {
  readonly outcome: MeasureLxmfTextOutcome | null;
  readonly failure: string | null;
}) {
  if (failure !== null) {
    return <BodyText muted>Native measurement unavailable: {failure}</BodyText>;
  }
  if (outcome === null) {
    return <BodyText muted>Measuring complete LXMF wire…</BodyText>;
  }
  switch (outcome.type) {
    case "measured":
      return (
        <BodyText muted>
          {outcome.wireBytes} encoded bytes · {outcome.remainingBytes} bytes remain for direct Link
          DATA
        </BodyText>
      );
    case "needsResource":
      return (
        <BodyText>
          {outcome.wireBytes} encoded bytes requires a Resource, which is not implemented.
        </BodyText>
      );
    case "invalidMessage":
      return <BodyText>Native LXMF measurement rejected this message.</BodyText>;
    case "localNodeStopped":
      return <BodyText>Start the local node before measuring.</BodyText>;
    case "busy":
      return <BodyText>Native LXMF command lane is busy; measurement will retry on edit.</BodyText>;
  }
}

function SendResult({ outcome }: { readonly outcome: SendDirectTextOutcome | null }) {
  if (outcome === null) {
    return null;
  }
  switch (outcome.type) {
    case "accepted":
      return (
        <BodyText>
          Saved record {outcome.localRecordId.toString()} to the durable queue. Delivery still
          requires transport proof.
        </BodyText>
      );
    case "needsResource":
      return <BodyText>{outcome.wireBytes} bytes needs an unsupported Resource carrier.</BodyText>;
    case "unsupportedRemoteStampRequirement":
      return (
        <BodyText>
          This peer requires unsupported LXMF stamp cost {outcome.requiredStampCost.toString()}.
        </BodyText>
      );
    case "peerIdentityUnavailable":
      return <BodyText>No compatible authenticated announce is available for this peer.</BodyText>;
    case "developmentUnavailable":
      return <BodyText>Durable send unavailable: {outcome.detail}</BodyText>;
    case "developmentResetRequired":
      return <BodyText>Reset development data before sending: {outcome.reason}</BodyText>;
  }
}

function mailboxMutationLabel(
  kind: "retry" | "cancel",
  outcome: RetryLxmfMessageOutcome | CancelLxmfMessageOutcome,
): string {
  switch (outcome.type) {
    case "accepted":
      return `Record ${outcome.localRecordId.toString()} was requeued with its exact stored wire.`;
    case "cancelled":
      return `Record ${outcome.localRecordId.toString()} was cancelled.`;
    case "notFound":
      return `The record no longer exists, so ${kind} was not applied.`;
    case "notFailed":
      return `Retry was not applied because the record is ${outcome.current.type}.`;
    case "alreadyDelivered":
      return "Cancellation lost the race to transport proof; the record is delivered.";
    case "alreadyCancelled":
      return "The record was already cancelled.";
    case "notCancellable":
      return `Cancellation was not applied because the record is ${outcome.current.type}.`;
    case "developmentUnavailable":
      return `Durable mailbox ${kind} unavailable: ${outcome.detail}`;
    case "developmentResetRequired":
      return `Reset development data before mailbox ${kind}: ${outcome.reason}`;
  }
}

function LxmfHealthCard() {
  const development = useDevelopmentRuntime();
  const health = development.snapshot?.lxmf;
  const state =
    health?.state ??
    (development.availability.type !== "available"
      ? "Unavailable"
      : development.phase === "starting"
        ? "Starting"
        : "Stopped");
  return (
    <Card>
      <Subheading>LXMF service</Subheading>
      <KeyValue label="State" value={state} />
      <KeyValue
        label="Inbound callback overflow"
        value={health?.inboundOverflowCount.toString() ?? "0"}
      />
      {health?.state === "degraded" ? (
        <BodyText>
          LXMF processing or durable mailbox access is degraded; authoritative records may lag until
          the condition recovers.
        </BodyText>
      ) : null}
    </Card>
  );
}

function conversationDestinations(
  peers: readonly LxmfPeerSummary[],
  messages: readonly LxmfMessage[],
): readonly DestinationHash[] {
  const destinations = new Map<string, DestinationHash>();
  for (const peer of peers) {
    destinations.set(formatContactHash(peer.destination), peer.destination);
  }
  for (const message of messages) {
    const destination = messagePeer(message);
    destinations.set(formatContactHash(destination), destination);
  }
  return [...destinations.values()].sort((left, right) =>
    formatContactHash(left).localeCompare(formatContactHash(right)),
  );
}

function applyPeerResult(
  result: RuntimeCommandResult<LxmfPeerListOutcome>,
  publish: (peers: readonly LxmfPeerSummary[]) => void,
  fail: (detail: string) => void,
): void {
  if (result.type === "operationFailure") {
    fail(result.detail);
  } else if (result.outcome.type === "listed") {
    publish(result.outcome.peers);
  } else {
    fail(
      result.outcome.type === "busy"
        ? "The bounded LXMF query lane is busy."
        : "The local node is stopped.",
    );
  }
}

function applyMessageResult(
  result: RuntimeCommandResult<LxmfMessageListOutcome>,
  publish: (messages: readonly LxmfMessage[]) => void,
  fail: (detail: string) => void,
): void {
  if (result.type === "operationFailure") {
    fail(result.detail);
  } else if (result.outcome.type === "listed") {
    publish(result.outcome.messages);
  } else if (result.outcome.type === "invalidInput") {
    fail(result.outcome.detail);
  } else if (result.outcome.type === "developmentUnavailable") {
    fail(result.outcome.detail);
  } else if (result.outcome.type === "developmentResetRequired") {
    fail(`Reset development data to reopen the mailbox: ${result.outcome.reason}`);
  }
}

function applyContactResult(
  outcome: ContactListOutcome,
  publish: (contacts: readonly Contact[]) => void,
): void {
  if (outcome.type === "listed") {
    publish(outcome.contacts);
  }
}

function announceOutcomeLabel(outcome: AnnounceLxmfOutcome): string {
  switch (outcome.type) {
    case "announced":
      return "The local lxmf.delivery destination was announced.";
    case "localNodeStopped":
      return "The local node stopped before the announcement.";
    case "busy":
      return "The bounded native LXMF command lane is busy.";
    case "failed":
      return "The local node rejected the LXMF announcement.";
  }
}

function UnavailableCard({ platform }: { readonly platform: string }) {
  return (
    <Card>
      <Badge tone="warning">iOS only</Badge>
      <BodyText>
        The native LXMF aggregate is not implemented for {platform}. No synthetic messages are
        shown.
      </BodyText>
    </Card>
  );
}

function FailureCard({ detail }: { readonly detail: string }) {
  return (
    <Card>
      <Badge tone="warning">LXMF unavailable</Badge>
      <BodyText>{detail}</BodyText>
    </Card>
  );
}

function formatThrown(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const styles = StyleSheet.create({
  actions: { gap: space.sm },
  input: {
    borderRadius: radius.sm,
    borderWidth: 1,
    fontSize: 16,
    minHeight: 48,
    paddingHorizontal: space.md,
    paddingVertical: space.sm,
  },
  messageInput: {
    minHeight: 112,
    textAlignVertical: "top",
  },
});
