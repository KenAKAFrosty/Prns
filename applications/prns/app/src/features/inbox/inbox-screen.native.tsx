import { StoragePreparationFailure } from "@/native/storage-preparation-failure";
import * as Bindings from "@prns-internal/expo";
import type {
  CancelLxmfMessageOutcome,
  DevelopmentNodeSnapshot,
  LxmfMessage,
  MeasureLxmfTextOutcome,
  RetryLxmfMessageOutcome,
  SendDirectTextOutcome,
} from "@prns-internal/expo";
import type { Href } from "expo-router";
import { useRouter } from "expo-router";
import { useEffect, useRef, useState } from "react";
import { Text } from "react-native";

import { formatContactHash, parseDestinationHash } from "@/features/contacts/format";
import { lastHeardLabel, useMessagingDirectory } from "@/features/contacts/messaging-directory";
import {
  type RuntimeCommandResult,
  useDevelopmentRuntime,
} from "@/native/development-runtime-context";
import { NavigationLink } from "@/ui/navigation-link";
import {
  Badge,
  ActionRow,
  BodyText,
  Button,
  Card,
  CardHeader,
  CardSection,
  KeyValue,
  Screen,
  ScreenHeading,
  Subheading,
} from "@/ui/primitives";
import { TextField } from "@/ui/text-field";
import {
  deliveryLabel,
  hasUnverifiedSender,
  messagePeer,
  peerLabel,
  textPresentation,
  timestampLabel,
  verificationLabel,
} from "./format";
import { useLxmfData } from "./use-lxmf-data";

export function InboxScreen() {
  const development = useDevelopmentRuntime();
  const data = useLxmfData(null);
  const nodeRunning =
    development.phase === "ready" &&
    development.snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Running;

  return (
    <Screen>
      <ScreenHeading>Inbox</ScreenHeading>
      <BodyText muted>Keep prns open for reliable message delivery.</BodyText>
      <LxmfHealthCard />
      {development.availability.type !== "available" ? (
        <UnavailableCard platform={development.availability.platform} />
      ) : (
        <>
          {nodeRunning ? null : <MessagingOfflineCard />}
          <ActionRow>
            {nodeRunning ? (
              <NavigationLink href="/inbox/compose">New message</NavigationLink>
            ) : null}
            <Button disabled={data.pending} onPress={() => void data.refresh()} tone="secondary">
              {data.pending ? "Refreshing…" : "Refresh Inbox"}
            </Button>
          </ActionRow>
          {data.failure === null ? null : <FailureCard detail={data.failure} />}
          {!data.messagesLoaded && data.failure === null ? (
            <Card>
              <BodyText muted>Loading saved messages…</BodyText>
            </Card>
          ) : null}
          {data.messages.length === 0 ? (
            data.messagesLoaded && data.failure === null && !data.pending ? (
              <Card>
                <Badge>No conversations</Badge>
                <BodyText>
                  {nodeRunning
                    ? "Start a new message or discover people in Contacts."
                    : "No messages are saved on this device."}
                </BodyText>
              </Card>
            ) : null
          ) : (
            data.messages.map((latest) => {
              const destination = messagePeer(latest);
              const encoded = formatContactHash(destination);
              const compatiblePeer = data.peers.find(
                (peer) => formatContactHash(peer.destination) === encoded,
              );
              const label = peerLabel(destination, data.peers, data.contacts);
              const unverified = hasUnverifiedSender(latest);
              const href: Href = {
                pathname: "/inbox/conversation/[destination]",
                params: { destination: encoded },
              };
              return (
                <Card key={encoded}>
                  {unverified ? <Badge tone="warning">{verificationLabel(latest)}</Badge> : null}
                  <Subheading>{unverified ? `Claims to be ${label}` : label}</Subheading>
                  <KeyValue
                    label="Last seen"
                    value={
                      compatiblePeer === undefined
                        ? "Not recently seen"
                        : lastHeardLabel(compatiblePeer.lastObservedAgeMillis)
                    }
                  />
                  <BodyText muted>
                    {textPresentation(latest.content).text} · {deliveryLabel(latest)}
                  </BodyText>
                  <NavigationLink href={href}>Open conversation</NavigationLink>
                </Card>
              );
            })
          )}
          {data.hasMore ? (
            <Button disabled={data.pending} tone="secondary" onPress={() => void data.loadOlder()}>
              {data.pending ? "Loading…" : "Load older conversations"}
            </Button>
          ) : null}
          {nodeRunning ? null : (
            <BodyText muted>
              Open Nodes to check this device&apos;s connection before writing a new message.
            </BodyText>
          )}
        </>
      )}
    </Screen>
  );
}

export function ConversationScreen({ destination }: { readonly destination: Uint8Array }) {
  return <ConversationJourney key={formatContactHash(destination)} destination={destination} />;
}

function ConversationJourney({ destination }: { readonly destination: Uint8Array }) {
  const development = useDevelopmentRuntime();
  const data = useLxmfData(destination);
  const [mutationStatus, setMutationStatus] = useState<string | null>(null);
  const [activeMutationId, setActiveMutationId] = useState<bigint | null>(null);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
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
    if (!mounted.current) return;
    setMutationStatus(
      result.type === "operationFailure"
        ? `Could not ${kind} this message. Try again.`
        : mailboxMutationLabel(kind, result.outcome),
    );
    await data.refresh();
    if (mounted.current) setActiveMutationId(null);
  };

  if (development.availability.type !== "available") {
    return (
      <Screen>
        <ScreenHeading>Conversation</ScreenHeading>
        <UnavailableCard platform={development.availability.platform} />
        <NavigationLink href="/inbox" direction="back">
          Back to Inbox
        </NavigationLink>
      </Screen>
    );
  }
  return (
    <Screen>
      <Badge>Messages</Badge>
      <ScreenHeading>{peerLabel(destination, data.peers, data.contacts)}</ScreenHeading>
      <KeyValue label="Destination" value={encoded} />
      {nodeRunning ? null : <MessagingOfflineCard />}
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
        <BodyText muted>
          Open Nodes to check this device&apos;s connection before writing a new message.
        </BodyText>
      )}
      <Subheading>Messages</Subheading>
      {!data.messagesLoaded && data.failure === null ? (
        <Card>
          <BodyText muted>Loading saved messages…</BodyText>
        </Card>
      ) : null}
      {data.messages.length === 0 ? (
        data.messagesLoaded && data.failure === null && !data.pending ? (
          <Card>
            <BodyText muted>No messages with this contact yet.</BodyText>
          </Card>
        ) : null
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
      {data.hasMore ? (
        <Button disabled={data.pending} tone="secondary" onPress={() => void data.loadOlder()}>
          {data.pending ? "Loading…" : "Load older messages"}
        </Button>
      ) : null}
      <NavigationLink href="/inbox" direction="back">
        Back to Inbox
      </NavigationLink>
    </Screen>
  );
}

export function ComposeScreen({
  initialDestination,
}: {
  readonly initialDestination: string | null;
}) {
  // A reused route is a new recipient intent; old native work may finish, but
  // its departed composer must not navigate or write into the new form.
  return (
    <ComposeJourney key={initialDestination ?? "choose"} initialDestination={initialDestination} />
  );
}

function ComposeJourney({ initialDestination }: { readonly initialDestination: string | null }) {
  const development = useDevelopmentRuntime();
  const router = useRouter();
  const directory = useMessagingDirectory();
  const [destinationText, setDestinationText] = useState(initialDestination ?? "");
  const [recipientMode, setRecipientMode] = useState<"saved" | "discovered" | "manual">("saved");
  const [sending, setSending] = useState(false);
  const destination = parseDestinationHash(destinationText);
  const nodeRunning = directory.nodeRunning;
  const savedRecipients = directory.contacts.filter((contact) => contact.isMessaging);
  const discoveredRecipients = directory.peers.filter(
    (peer) =>
      !savedRecipients.some(
        (contact) => formatContactHash(contact.destination) === formatContactHash(peer.destination),
      ),
  );

  if (development.availability.type !== "available") {
    return (
      <Screen>
        <ScreenHeading>Compose</ScreenHeading>
        <UnavailableCard platform={development.availability.platform} />
        <NavigationLink href="/inbox" direction="back">
          Back to Inbox
        </NavigationLink>
      </Screen>
    );
  }
  if (!nodeRunning) {
    return (
      <Screen>
        <ScreenHeading>Compose</ScreenHeading>
        <MessagingOfflineCard />
        <NavigationLink href="/inbox" direction="back">
          Back to Inbox
        </NavigationLink>
      </Screen>
    );
  }

  return (
    <Screen>
      <Badge>New message</Badge>
      <ScreenHeading>Compose</ScreenHeading>
      {destination !== null ? (
        <Card>
          <CardHeader title={peerLabel(destination, directory.peers, directory.contacts)} />
          <BodyText muted>{formatContactHash(destination)}</BodyText>
          <Button disabled={sending} tone="secondary" onPress={() => setDestinationText("")}>
            Change recipient
          </Button>
        </Card>
      ) : (
        <>
          <BodyText>Choose who to message.</BodyText>
          <ActionRow>
            <Button
              accessibilityState={{ selected: recipientMode === "saved" }}
              tone={recipientMode === "saved" ? "primary" : "secondary"}
              onPress={() => setRecipientMode("saved")}
            >
              Saved
            </Button>
            <Button
              accessibilityState={{ selected: recipientMode === "discovered" }}
              tone={recipientMode === "discovered" ? "primary" : "secondary"}
              onPress={() => setRecipientMode("discovered")}
            >
              Discovered
            </Button>
            <Button
              accessibilityState={{ selected: recipientMode === "manual" }}
              tone={recipientMode === "manual" ? "primary" : "secondary"}
              onPress={() => setRecipientMode("manual")}
            >
              Enter address
            </Button>
          </ActionRow>
          {directory.failure === null ? null : <FailureCard detail={directory.failure} />}
          {recipientMode === "manual" ? (
            <>
              <TextField
                label="Recipient"
                autoCapitalize="none"
                autoCorrect={false}
                onChangeText={setDestinationText}
                placeholder="32 hexadecimal characters"
                value={destinationText}
              />
              {destinationText.length === 0 ? null : (
                <BodyText>Enter exactly 32 hexadecimal characters.</BodyText>
              )}
            </>
          ) : (
            <>
              {(recipientMode === "saved" ? savedRecipients : discoveredRecipients).map(
                (recipient) => {
                  const encoded = formatContactHash(recipient.destination);
                  const label = peerLabel(
                    recipient.destination,
                    directory.peers,
                    directory.contacts,
                  );
                  return (
                    <Card key={encoded}>
                      <CardHeader title={label} />
                      <BodyText muted>{encoded}</BodyText>
                      <Button
                        accessibilityLabel={`Choose ${label}`}
                        onPress={() => setDestinationText(encoded)}
                      >
                        Choose recipient
                      </Button>
                    </Card>
                  );
                },
              )}
              {recipientMode === "saved" &&
              savedRecipients.length === 0 &&
              directory.failure === null ? (
                <BodyText>
                  {directory.pending
                    ? "Loading saved contacts…"
                    : "No saved messaging contacts. Discover someone in Contacts, or enter an address."}
                </BodyText>
              ) : null}
              {recipientMode === "discovered" && discoveredRecipients.length === 0 ? (
                <BodyText>
                  {directory.discoveryPending
                    ? "Loading discovered contacts…"
                    : "No other discovered messaging contacts. Ask someone to announce themselves, then refresh."}
                </BodyText>
              ) : null}
              {recipientMode === "discovered" && directory.discoveryFailure !== null ? (
                <BodyText>{directory.discoveryFailure}</BodyText>
              ) : null}
              <Button
                disabled={directory.pending || directory.discoveryPending}
                tone="secondary"
                onPress={() => void directory.refresh()}
              >
                Refresh recipients
              </Button>
              <NavigationLink href="/contacts">Open Contacts</NavigationLink>
            </>
          )}
        </>
      )}
      {destination === null ? null : (
        <>
          <Composer
            key={formatContactHash(destination)}
            destination={destination}
            onBusyChange={setSending}
            onSettled={async (result) => {
              if (
                result.type !== "outcome" ||
                result.outcome.tag !== Bindings.SendDirectTextOutcome_Tags.Accepted
              )
                return;
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
      <NavigationLink href="/inbox" direction="back">
        Back to Inbox
      </NavigationLink>
    </Screen>
  );
}

function Composer({
  destination,
  onSettled,
  onBusyChange,
}: {
  readonly destination: Uint8Array;
  readonly onBusyChange?: (busy: boolean) => void;
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
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      onBusyChange?.(false);
    };
  }, [onBusyChange]);

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
    onBusyChange?.(true);
    setSendOutcome(null);
    setSendFailure(null);
    const result = await development.sendDirectText({
      destination,
      title: nextTitle,
      content: nextContent,
    });
    if (!mounted.current) return;
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
    onBusyChange?.(false);
    await development.refreshSnapshot();
    if (mounted.current) await onSettled(result);
  };

  return (
    <Card>
      <Subheading>New message</Subheading>
      <TextField
        label="Title (optional)"
        editable={!sending}
        onChangeText={setTitle}
        placeholder="Title"
        value={title}
      />
      <TextField
        label="Message"
        editable={!sending}
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
        {sending ? "Finding contact…" : "Send message"}
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
  const unverified = hasUnverifiedSender(message);
  const [showDetails, setShowDetails] = useState(false);
  const direction = message.direction === Bindings.LxmfDirection.Inbound ? "Received" : "Sent";
  const detailsLabel = title.text.length === 0 ? timestampLabel(message.timestamp) : title.text;
  const directionBadge = (
    <Badge tone={!title.validUtf8 || !content.validUtf8 ? "warning" : "neutral"}>{direction}</Badge>
  );

  return (
    <Card>
      {unverified ? <Badge tone="warning">{verificationLabel(message)}</Badge> : null}
      {title.text.length === 0 ? (
        directionBadge
      ) : (
        <CardHeader title={title.text}>{directionBadge}</CardHeader>
      )}
      <BodyText>{content.text}</BodyText>
      <BodyText muted>
        {timestampLabel(message.timestamp)} · <Text>{deliveryLabel(message)}</Text>
      </BodyText>
      <ActionRow>
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
        <Button
          accessibilityLabel={`${showDetails ? "Hide" : "Show"} details for message ${detailsLabel}`}
          accessibilityState={{ expanded: showDetails }}
          onPress={() => setShowDetails((current) => !current)}
          tone="secondary"
        >
          {showDetails ? "Hide details" : "Message details"}
        </Button>
      </ActionRow>
      {showDetails ? (
        <CardSection>
          {unverified ? null : <KeyValue label="Verification" value={verificationLabel(message)} />}
          <KeyValue label="Message ID" value={formatContactHash(message.messageId)} />
        </CardSection>
      ) : null}
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
    case Bindings.SendDirectTextOutcome_Tags.RecipientUnavailable:
      return (
        <BodyText>
          This contact could not be found. Check your connection and try again. Your message has not
          been queued.
        </BodyText>
      );
    case Bindings.SendDirectTextOutcome_Tags.IdentityConflict:
      return (
        <BodyText>
          This contact's identity differs from the one you saved. Check the contact before sending.
          Your message has not been queued.
        </BodyText>
      );
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

function MessagingOfflineCard() {
  const development = useDevelopmentRuntime();
  const accessNeeded =
    development.bluetoothAuthorization?.authorization === "denied" ||
    development.bluetoothAuthorization?.authorization === "restricted";
  return (
    <Card>
      <Badge tone="warning">
        {accessNeeded
          ? "Bluetooth access needed"
          : development.phase === "starting"
            ? "Getting ready"
            : "Messaging offline"}
      </Badge>
      <BodyText>
        You can still read saved messages, retry failed messages, or cancel queued messages.
      </BodyText>
      {accessNeeded ? <NavigationLink href="/nodes">Check Bluetooth access</NavigationLink> : null}
    </Card>
  );
}

function LxmfHealthCard() {
  const development = useDevelopmentRuntime();
  const health = development.snapshot?.lxmf;
  if (
    health === undefined &&
    (development.bluetoothAuthorization?.authorization === "denied" ||
      development.bluetoothAuthorization?.authorization === "restricted")
  ) {
    return null;
  }
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
