import { StoragePreparationFailure } from "@/native/storage-preparation-failure";
import * as Bindings from "@prns-internal/expo";
import type {
  AnnounceLxmfOutcome,
  CancelLxmfMessageOutcome,
  Contact,
  ContactListOutcome,
  DevelopmentNodeSnapshot,
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
import { useCallback, useEffect, useMemo, useState } from "react";
import { StyleSheet, View } from "react-native";

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
import { TextField } from "@/ui/text-field";
import { space } from "@/ui/theme";
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
  readonly failure: string | Bindings.NativeStoragePreparationError | null;
  readonly refresh: () => Promise<void>;
};

function useLxmfData(peer: Uint8Array | null): LxmfData {
  const development = useDevelopmentRuntime();
  const contactRuntime = useContactRuntime();
  const [peers, setPeers] = useState<readonly LxmfPeerSummary[]>([]);
  const [messages, setMessages] = useState<readonly LxmfMessage[]>([]);
  const [contacts, setContacts] = useState<readonly Contact[]>([]);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | Bindings.NativeStoragePreparationError | null>(
    null,
  );
  const peerKey = peer === null ? null : formatContactHash(peer);
  const nodeRunning =
    development.phase === "ready" &&
    development.snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Running;

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
      development.listLxmfMessages({
        peer: selectedPeer ?? undefined,
        before: undefined,
        limit: pageLimit,
      }),
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
      } catch (failure) {
        setFailure(
          failure instanceof Bindings.NativeStoragePreparationError
            ? failure
            : "Contacts could not be loaded.",
        );
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
  const nodeRunning =
    development.phase === "ready" &&
    development.snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Running;

  const announce = async () => {
    setAnnouncing(true);
    setCommand(null);
    const result = await development.announceLxmf();
    setCommand(
      result.type === "operationFailure"
        ? "The messaging address could not be shared."
        : announceOutcomeLabel(result.outcome),
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
      <Badge>Messages</Badge>
      <ScreenHeading>Inbox</ScreenHeading>
      <BodyText>Your messages are saved on this device, including while offline.</BodyText>
      <BodyText muted>Keep prns open for reliable message delivery.</BodyText>
      <LxmfHealthCard />
      {development.availability.type !== "available" ? (
        <UnavailableCard platform={development.availability.platform} />
      ) : (
        <>
          {nodeRunning ? null : (
            <Card>
              <Badge tone="warning">
                {development.phase === "starting" ? "Getting ready" : "Messaging offline"}
              </Badge>
              <BodyText>
                {development.phase === "starting"
                  ? "Messaging will be available in a moment."
                  : "Saved messages remain available. See Nodes > This device for diagnostic details."}
              </BodyText>
            </Card>
          )}
          <View style={styles.actions}>
            {nodeRunning ? (
              <NavigationLink href="/inbox/compose">New message</NavigationLink>
            ) : null}
            {nodeRunning ? (
              <>
                <BodyText muted>
                  Before exchanging messages with a new contact, share this device&apos;s messaging
                  address.
                </BodyText>
                <Button disabled={announcing} onPress={() => void announce()}>
                  {announcing ? "Sharing…" : "Share messaging address"}
                </Button>
              </>
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
                  ? "Share your messaging address or start a new message."
                  : "No messages are saved on this device."}
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
                    label="Last seen"
                    value={
                      compatiblePeer === undefined
                        ? "Not recently seen"
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
          {nodeRunning ? null : (
            <BodyText muted>
              Start this device&apos;s node to find contacts or write a new message.
            </BodyText>
          )}
        </>
      )}
    </Screen>
  );
}

export function ConversationScreen({ destination }: { readonly destination: Uint8Array }) {
  const development = useDevelopmentRuntime();
  const data = useLxmfData(destination);
  const [mutationStatus, setMutationStatus] = useState<string | null>(null);
  const [activeMutationId, setActiveMutationId] = useState<bigint | null>(null);
  const encoded = formatContactHash(destination);
  const peer = data.peers.find((candidate) => formatContactHash(candidate.destination) === encoded);
  const nodeRunning =
    development.phase === "ready" &&
    development.snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Running;

  const mutate = async (kind: "retry" | "cancel", localRecordId: bigint): Promise<void> => {
    setActiveMutationId(localRecordId);
    setMutationStatus(null);
    const result =
      kind === "retry"
        ? await development.retryLxmfMessage(localRecordId)
        : await development.cancelLxmfMessage(localRecordId);
    setMutationStatus(
      result.type === "operationFailure"
        ? `Could not ${kind} this message. Try again.`
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
      <Badge>Messages</Badge>
      <ScreenHeading>{peerLabel(destination, data.peers, data.contacts)}</ScreenHeading>
      <KeyValue label="Destination" value={encoded} />
      {nodeRunning ? null : (
        <Card>
          <Badge tone="warning">
            {development.phase === "starting" ? "Getting ready" : "Messaging offline"}
          </Badge>
          <BodyText>
            {development.phase === "starting"
              ? "Messaging will be available in a moment."
              : "You can still read saved messages, retry failed messages, or cancel queued messages."}
          </BodyText>
        </Card>
      )}
      {peer?.requiredStampCost === undefined ? null : (
        <Card>
          <Badge tone="warning">Sending unavailable</Badge>
          <BodyText>
            This contact requires a messaging feature that prns does not support yet.
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
        <BodyText muted>Start this device&apos;s node to compose a new message.</BodyText>
      )}
      <Subheading>Messages</Subheading>
      {data.messages.length === 0 ? (
        <Card>
          <BodyText muted>No messages with this contact yet.</BodyText>
        </Card>
      ) : (
        data.messages.map((message) => (
          <MessageCard
            key={message.localRecordId.toString()}
            message={message}
            pending={activeMutationId === message.localRecordId}
            onRetry={
              message.deliveryState.tag === Bindings.LxmfDeliveryState_Tags.Failed
                ? () => mutate("retry", message.localRecordId)
                : undefined
            }
            onCancel={
              message.deliveryState.tag === Bindings.LxmfDeliveryState_Tags.Queued ||
              message.deliveryState.tag === Bindings.LxmfDeliveryState_Tags.Sending
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
  const nodeRunning =
    development.phase === "ready" &&
    development.snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Running;

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
          <Badge>{development.phase === "starting" ? "Getting ready" : "Messaging offline"}</Badge>
          <BodyText>
            Start this device&apos;s node before writing a message. See Nodes &gt; This device for
            diagnostic details.
          </BodyText>
        </Card>
        <NavigationLink href="/inbox">Back to Inbox</NavigationLink>
      </Screen>
    );
  }

  return (
    <Screen>
      <Badge>New message</Badge>
      <ScreenHeading>Compose</ScreenHeading>
      <BodyText>
        Enter the 32-character destination for the person or device you want to reach.
      </BodyText>
      <TextField
        label="Recipient"
        autoCapitalize="none"
        autoCorrect={false}
        onChangeText={setDestinationText}
        placeholder="32 hexadecimal characters"
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
              if (
                result.type !== "outcome" ||
                result.outcome.tag !== Bindings.SendDirectTextOutcome_Tags.Accepted
              ) {
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
  readonly destination: Uint8Array;
  readonly onSettled: (result: RuntimeCommandResult<SendDirectTextOutcome>) => Promise<void>;
}) {
  const development = useDevelopmentRuntime();
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
        setMeasureFailure("Try editing the message again.");
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
      setSendFailure("Try again.");
    } else {
      setSendOutcome(result.outcome);
      if (result.outcome.tag === Bindings.SendDirectTextOutcome_Tags.Accepted) {
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
      <TextField
        label="Title (optional)"
        onChangeText={setTitle}
        placeholder="Title"
        value={title}
      />
      <TextField
        label="Message"
        multiline
        onChangeText={setContent}
        placeholder="Message"
        value={content}
      />
      <Measurement outcome={measurement} failure={measureFailure} />
      <Button
        disabled={
          sending ||
          content.length === 0 ||
          measurement === null ||
          measurement.tag !== Bindings.MeasureLxmfTextOutcome_Tags.Measured
        }
        onPress={() => void send()}
      >
        {sending ? "Sending…" : "Send message"}
      </Button>
      {sendFailure === null ? null : <BodyText>Could not send: {sendFailure}</BodyText>}
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
  const unverified = message.verification !== Bindings.LxmfVerification.Verified;

  return (
    <Card>
      <Badge tone={unverified || !title.validUtf8 || !content.validUtf8 ? "warning" : "neutral"}>
        {message.direction === Bindings.LxmfDirection.Inbound ? "Received" : "Sent"}
      </Badge>
      <Subheading>{title.text.length === 0 ? "Untitled" : title.text}</Subheading>
      <BodyText>{content.text}</BodyText>
      <KeyValue label="Time" value={timestampLabel(message.timestamp)} />
      <KeyValue label="Verification" value={verificationLabel(message)} />
      <KeyValue label="Delivery" value={deliveryLabel(message)} />
      <KeyValue label="Message ID" value={formatContactHash(message.messageId)} />
      {onRetry === undefined ? null : (
        <Button disabled={pending} onPress={() => void onRetry()} tone="secondary">
          {pending ? "Retrying…" : "Retry message"}
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
    return <BodyText muted>Could not check message size: {failure}</BodyText>;
  }
  if (outcome === null) {
    return <BodyText muted>Checking message size…</BodyText>;
  }
  switch (outcome.tag) {
    case Bindings.MeasureLxmfTextOutcome_Tags.Measured:
      return (
        <BodyText muted>
          {outcome.inner.wireBytes} bytes used · {outcome.inner.remainingBytes} bytes available
        </BodyText>
      );
    case Bindings.MeasureLxmfTextOutcome_Tags.NeedsResource:
      return (
        <BodyText>
          {outcome.inner.wireBytes} bytes used. This message is too large for direct delivery.
        </BodyText>
      );
    case Bindings.MeasureLxmfTextOutcome_Tags.InvalidMessage:
      return <BodyText>This message cannot be sent.</BodyText>;
    case Bindings.MeasureLxmfTextOutcome_Tags.LocalNodeStopped:
      return <BodyText>Start this device&apos;s node before sending.</BodyText>;
    case Bindings.MeasureLxmfTextOutcome_Tags.Busy:
      return <BodyText>Another messaging action is in progress.</BodyText>;
  }
}

function SendResult({ outcome }: { readonly outcome: SendDirectTextOutcome | null }) {
  if (outcome === null) {
    return null;
  }
  switch (outcome.tag) {
    case Bindings.SendDirectTextOutcome_Tags.Accepted:
      return <BodyText>Message queued.</BodyText>;
    case Bindings.SendDirectTextOutcome_Tags.NeedsResource:
      return <BodyText>This message is too large for direct delivery.</BodyText>;
    case Bindings.SendDirectTextOutcome_Tags.UnsupportedRemoteStampRequirement:
      return (
        <BodyText>
          This contact requires a messaging feature that prns does not support yet.
        </BodyText>
      );
    case Bindings.SendDirectTextOutcome_Tags.PeerIdentityUnavailable:
      return <BodyText>This address is not ready to receive messages.</BodyText>;
    case Bindings.SendDirectTextOutcome_Tags.DevelopmentUnavailable:
      return <BodyText>Sending is not available right now.</BodyText>;
    case Bindings.SendDirectTextOutcome_Tags.DevelopmentResetRequired:
      return <BodyText>Reset app data before sending.</BodyText>;
  }
}

function mailboxMutationLabel(
  kind: "retry" | "cancel",
  outcome: RetryLxmfMessageOutcome | CancelLxmfMessageOutcome,
): string {
  switch (outcome.tag) {
    case Bindings.RetryLxmfMessageOutcome_Tags.Accepted:
      return "Message queued to retry.";
    case Bindings.CancelLxmfMessageOutcome_Tags.Cancelled:
      return "Message cancelled.";
    case Bindings.RetryLxmfMessageOutcome_Tags.NotFound:
    case Bindings.CancelLxmfMessageOutcome_Tags.NotFound:
      return `The message no longer exists, so it could not be ${kind === "retry" ? "retried" : "cancelled"}.`;
    case Bindings.RetryLxmfMessageOutcome_Tags.NotFailed:
      return "Only failed messages can be retried.";
    case Bindings.CancelLxmfMessageOutcome_Tags.AlreadyDelivered:
      return "This message was delivered before it could be cancelled.";
    case Bindings.CancelLxmfMessageOutcome_Tags.AlreadyCancelled:
      return "This message was already cancelled.";
    case Bindings.CancelLxmfMessageOutcome_Tags.NotCancellable:
      return "Only queued or sending messages can be cancelled.";
    case Bindings.RetryLxmfMessageOutcome_Tags.DevelopmentUnavailable:
    case Bindings.CancelLxmfMessageOutcome_Tags.DevelopmentUnavailable:
      return `Could not ${kind} this message. Try again.`;
    case Bindings.RetryLxmfMessageOutcome_Tags.DevelopmentResetRequired:
    case Bindings.CancelLxmfMessageOutcome_Tags.DevelopmentResetRequired:
      return "Reset app data before trying again.";
  }
}

function LxmfHealthCard() {
  const development = useDevelopmentRuntime();
  const health = development.snapshot?.lxmf;
  if (health?.state === Bindings.LxmfHealthState.Ready) {
    return <Badge>Messaging ready</Badge>;
  }
  const state =
    health === undefined
      ? development.availability.type !== "available"
        ? "Unavailable"
        : development.phase === "starting"
          ? "Getting ready"
          : "Offline"
      : messagingStateLabel(health.state);
  return (
    <Card>
      <Subheading>Messaging status</Subheading>
      <KeyValue label="State" value={state} />
      {health?.state === Bindings.LxmfHealthState.Degraded ? (
        <BodyText>Messages may be delayed until the connection recovers.</BodyText>
      ) : null}
    </Card>
  );
}

function messagingStateLabel(state: NonNullable<DevelopmentNodeSnapshot["lxmf"]>["state"]): string {
  switch (state) {
    case Bindings.LxmfHealthState.Ready:
      return "Ready";
    case Bindings.LxmfHealthState.Degraded:
      return "Limited";
    case Bindings.LxmfHealthState.Stopped:
      return "Offline";
  }
}

function conversationDestinations(
  peers: readonly LxmfPeerSummary[],
  messages: readonly LxmfMessage[],
): readonly Uint8Array[] {
  const destinations = new Map<string, Uint8Array>();
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
  fail: (detail: string | Bindings.NativeStoragePreparationError) => void,
): void {
  if (result.type === "operationFailure") {
    fail("Contacts could not be found right now.");
  } else if (result.outcome.tag === Bindings.LxmfPeerListOutcome_Tags.Listed) {
    publish(result.outcome.inner.peers);
  } else {
    fail(
      result.outcome.tag === Bindings.LxmfPeerListOutcome_Tags.Busy
        ? "Another messaging action is in progress."
        : "Messaging is offline.",
    );
  }
}

function applyMessageResult(
  result: RuntimeCommandResult<LxmfMessageListOutcome>,
  publish: (messages: readonly LxmfMessage[]) => void,
  fail: (detail: string | Bindings.NativeStoragePreparationError) => void,
): void {
  if (result.type === "operationFailure") {
    fail(
      result.storagePreparation === undefined
        ? "Messages could not be loaded. Try again."
        : new Bindings.NativeStoragePreparationError(result.storagePreparation),
    );
  } else if (result.outcome.tag === Bindings.LxmfMessageListOutcome_Tags.Listed) {
    publish(result.outcome.inner.messages);
  } else if (result.outcome.tag === Bindings.LxmfMessageListOutcome_Tags.InvalidInput) {
    fail("Messages could not be loaded for this destination.");
  } else if (result.outcome.tag === Bindings.LxmfMessageListOutcome_Tags.DevelopmentUnavailable) {
    fail("Messages are not available right now.");
  } else if (result.outcome.tag === Bindings.LxmfMessageListOutcome_Tags.DevelopmentResetRequired) {
    fail("Reset app data to use messaging again.");
  }
}

function applyContactResult(
  outcome: ContactListOutcome,
  publish: (contacts: readonly Contact[]) => void,
): void {
  if (outcome.tag === Bindings.ContactListOutcome_Tags.Listed) {
    publish(outcome.inner.contacts);
  }
}

function announceOutcomeLabel(outcome: AnnounceLxmfOutcome): string {
  switch (outcome) {
    case Bindings.AnnounceLxmfOutcome.Announced:
      return "Messaging address shared.";
    case Bindings.AnnounceLxmfOutcome.LocalNodeStopped:
      return "This device went offline before its address could be shared.";
    case Bindings.AnnounceLxmfOutcome.Busy:
      return "Another messaging action is in progress.";
    case Bindings.AnnounceLxmfOutcome.Failed:
      return "The messaging address could not be shared.";
  }
}

function UnavailableCard({ platform }: { readonly platform: string }) {
  return (
    <Card>
      <Badge tone="warning">Messaging unavailable</Badge>
      <BodyText>Messaging is not available on {platform} yet.</BodyText>
    </Card>
  );
}

function FailureCard({
  detail,
}: {
  readonly detail: string | Bindings.NativeStoragePreparationError;
}) {
  if (detail instanceof Bindings.NativeStoragePreparationError) {
    return <StoragePreparationFailure outcome={detail.outcome} />;
  }
  return (
    <Card>
      <Badge tone="warning">Messaging unavailable</Badge>
      <BodyText>{detail}</BodyText>
    </Card>
  );
}

const styles = StyleSheet.create({
  actions: { gap: space.sm },
});
