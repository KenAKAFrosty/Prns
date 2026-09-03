import type {
  AnnounceLxmfOutcome,
  Contact,
  ContactListOutcome,
  LxmfMessage,
  LxmfMessageListOutcome,
  LxmfPeerListOutcome,
  LxmfPeerSummary,
  MeasureLxmfTextOutcome,
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

  const refresh = useCallback(async () => {
    if (development.phase !== "ready") {
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
      development.listLxmfPeers(),
      development.listLxmfMessages({ peer: selectedPeer, before: null, limit: pageLimit }),
    ]);
    applyPeerResult(peerResult, setPeers, setFailure);
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
    development.listLxmfMessages,
    development.listLxmfPeers,
    development.phase,
    peerKey,
  ]);

  useEffect(() => {
    if (development.snapshot?.revision !== undefined) {
      void refresh();
    }
  }, [development.snapshot?.revision, refresh]);

  return { peers, messages, contacts, pending, failure, refresh };
}

export function InboxScreen() {
  const development = useDevelopmentRuntime();
  const data = useLxmfData(null);
  const [command, setCommand] = useState<string | null>(null);
  const [announcing, setAnnouncing] = useState(false);

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
      <Badge>In-memory direct LXMF</Badge>
      <ScreenHeading>Inbox</ScreenHeading>
      <BodyText>
        Compatible announcements and messages belong to this running development generation.
        Restarting clears them; durable mailbox behavior is not implemented yet.
      </BodyText>
      <LxmfHealthCard />
      {development.availability.type !== "available" ? (
        <UnavailableCard platform={development.availability.platform} />
      ) : development.phase !== "ready" ? (
        <Card>
          <Badge>{development.phase}</Badge>
          <BodyText>{development.lifecycleFailure ?? "Starting the local node…"}</BodyText>
        </Card>
      ) : (
        <>
          <View style={styles.actions}>
            <Button disabled={announcing} onPress={() => void announce()}>
              {announcing ? "Announcing…" : "Announce LXMF destination"}
            </Button>
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
                Wait for a compatible lxmf.delivery announce from the controlled peer, then open the
                composer.
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
                  <KeyValue label="Messages this run" value={peerMessages.length.toString()} />
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
          <NavigationLink href="/inbox/compose">Compose by destination</NavigationLink>
        </>
      )}
    </Screen>
  );
}

export function ConversationScreen({ destination }: { readonly destination: DestinationHash }) {
  const development = useDevelopmentRuntime();
  const data = useLxmfData(destination);
  const [retryOutcome, setRetryOutcome] = useState<SendDirectTextOutcome | null>(null);
  const encoded = formatContactHash(destination);
  const peer = data.peers.find((candidate) => formatContactHash(candidate.destination) === encoded);
  if (development.availability.type !== "available") {
    return (
      <Screen>
        <ScreenHeading>Conversation</ScreenHeading>
        <UnavailableCard platform={development.availability.platform} />
        <NavigationLink href="/inbox">Back to Inbox</NavigationLink>
      </Screen>
    );
  }
  if (development.phase !== "ready") {
    return (
      <Screen>
        <ScreenHeading>Conversation</ScreenHeading>
        <Card>
          <Badge>{development.phase}</Badge>
          <BodyText>{development.lifecycleFailure ?? "Starting the local node…"}</BodyText>
        </Card>
        <NavigationLink href="/inbox">Back to Inbox</NavigationLink>
      </Screen>
    );
  }
  return (
    <Screen>
      <Badge>Direct conversation</Badge>
      <ScreenHeading>{peerLabel(destination, data.peers, data.contacts)}</ScreenHeading>
      <KeyValue label="Destination" value={encoded} />
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
      <SendResult outcome={retryOutcome} />
      <Composer destination={destination} onSettled={data.refresh} />
      <Subheading>Messages</Subheading>
      {data.messages.length === 0 ? (
        <Card>
          <BodyText muted>No messages for this destination in the current process.</BodyText>
        </Card>
      ) : (
        data.messages.map((message) => (
          <MessageCard
            key={message.localRecordId.toString()}
            message={message}
            onRetry={
              message.deliveryState === "failed"
                ? async () => {
                    if (message.title.type !== "utf8" || message.content.type !== "utf8") {
                      return;
                    }
                    const result = await development.sendDirectText({
                      destination,
                      title: message.title.value,
                      content: message.content.value,
                    });
                    if (result.type === "outcome") {
                      setRetryOutcome(result.outcome);
                    }
                    await development.refreshSnapshot();
                    await data.refresh();
                  }
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

  if (development.availability.type !== "available") {
    return (
      <Screen>
        <ScreenHeading>Compose</ScreenHeading>
        <UnavailableCard platform={development.availability.platform} />
        <NavigationLink href="/inbox">Back to Inbox</NavigationLink>
      </Screen>
    );
  }
  if (development.phase !== "ready") {
    return (
      <Screen>
        <ScreenHeading>Compose</ScreenHeading>
        <Card>
          <Badge>{development.phase}</Badge>
          <BodyText>{development.lifecycleFailure ?? "Starting the local node…"}</BodyText>
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
        wire; delivery remains proof-gated.
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
              if (result.type !== "outcome" || result.outcome.type !== "started") {
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
      if (result.outcome.type === "started") {
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
        {sending ? "Sending — awaiting proof…" : "Send direct message"}
      </Button>
      {sendFailure === null ? null : <BodyText>Native send unavailable: {sendFailure}</BodyText>}
      <SendResult outcome={sendOutcome} />
    </Card>
  );
}

function MessageCard({
  message,
  onRetry,
}: {
  readonly message: LxmfMessage;
  readonly onRetry: (() => Promise<void>) | undefined;
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
        <Button onPress={() => void onRetry()} tone="secondary">
          Retry as a new message
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
    case "started":
      return (
        <BodyText>
          Started record {outcome.localRecordId.toString()}. The proof-backed message state is
          authoritative.
        </BodyText>
      );
    case "needsResource":
      return <BodyText>{outcome.wireBytes} bytes needs an unsupported Resource carrier.</BodyText>;
    case "unsupportedRemoteStampRequirement":
      return <BodyText>This peer requires an unsupported LXMF stamp.</BodyText>;
    case "peerIdentityUnavailable":
      return <BodyText>No compatible authenticated announce is available for this peer.</BodyText>;
    case "noRoute":
      return <BodyText>No route was found. Retry creates a new message in this slice.</BodyText>;
    case "linkFailed":
      return <BodyText>The direct Link failed. Retry creates a new message.</BodyText>;
    case "deliveryTimedOut":
      return <BodyText>Transport proof timed out. Delivery is not claimed.</BodyText>;
    case "invalidMessage":
      return <BodyText>Native LXMF composition rejected this message.</BodyText>;
    case "localNodeStopped":
      return <BodyText>The local node stopped before this command was admitted.</BodyText>;
    case "busy":
      return <BodyText>The bounded native LXMF command lane is busy. Try again.</BodyText>;
  }
}

function LxmfHealthCard() {
  const development = useDevelopmentRuntime();
  const health = development.snapshot?.lxmf;
  return (
    <Card>
      <Subheading>LXMF service</Subheading>
      <KeyValue label="State" value={health?.state ?? "Starting"} />
      <KeyValue
        label="Inbound callback overflow"
        value={health?.inboundOverflowCount.toString() ?? "0"}
      />
      {health?.state === "degraded" ? (
        <BodyText>
          At least one proven inbound carrier could not enter the local processing lane.
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
  return [...destinations.values()].toSorted((left, right) =>
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
  } else {
    fail(
      result.outcome.type === "busy"
        ? "The bounded LXMF query lane is busy."
        : "The local node is stopped.",
    );
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
