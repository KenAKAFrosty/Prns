import * as Bindings from "@prns-internal/expo";
import type {
  Contact,
  ContactListOutcome,
  ContactLookupOutcome,
  ContactMutationOutcome,
  DevelopmentRuntime,
  LxmfPeerSummary,
} from "@prns-internal/expo";
import type { Href } from "expo-router";
import { useRouter } from "expo-router";
import type { DestinationHash } from "personal-rns/contract";
import { useCallback, useEffect, useState } from "react";

import { StoragePreparationFailure } from "@/native/storage-preparation-failure";
import { useContactRuntime } from "@/native/contact-runtime-context";
import { useDevelopmentRuntime } from "@/native/development-runtime-context";
import { peerLabel, shortDestination } from "@/features/inbox/format";
import { useMessagingDirectory, lastHeardLabel } from "./messaging-directory";
import { MessagingProfileCard } from "./messaging-profile";
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
import { formatContactHash, parseDestinationHash, parseIdentityHash } from "./format";

export function ContactsScreen() {
  const contactRuntime = useContactRuntime();
  return (
    <Screen>
      <ScreenHeading>Contacts</ScreenHeading>
      {contactRuntime.runtime === null ? (
        <NativeContactsUnavailable platform={contactRuntime.availability.platform} />
      ) : (
        <MessagingContacts />
      )}
    </Screen>
  );
}

function MessagingContacts() {
  const development = useDevelopmentRuntime();
  const directory = useMessagingDirectory();
  const [mode, setMode] = useState<"saved" | "discovered">("saved");
  const [savingProfile, setSavingProfile] = useState(false);
  const [command, setCommand] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [mutation, setMutation] = useState<ContactMutationOutcome | null>(null);
  const [saveFailure, setSaveFailure] = useState<
    string | Bindings.NativeStoragePreparationError | null
  >(null);

  const announce = async () => {
    setPendingAction("announce");
    const result = await development.announceLxmf();
    setCommand(
      result.type === "operationFailure"
        ? "The announcement could not be confirmed. Try again."
        : announceLabel(result.outcome),
    );
    setPendingAction(null);
  };
  const save = async (destination: Uint8Array) => {
    setPendingAction(formatContactHash(destination));
    setMutation(null);
    setSaveFailure(null);
    const result = await development.saveDiscoveredContact({ destination });
    if (result.type === "operationFailure") {
      setSaveFailure(
        result.storagePreparation === undefined
          ? "The contact could not be saved. Try again."
          : new Bindings.NativeStoragePreparationError(result.storagePreparation),
      );
    } else {
      setMutation(result.outcome);
      if (
        result.outcome.tag === Bindings.ContactMutationOutcome_Tags.Saved ||
        result.outcome.tag === Bindings.ContactMutationOutcome_Tags.Existing ||
        result.outcome.tag === Bindings.ContactMutationOutcome_Tags.Updated
      ) {
        await directory.refreshContacts();
      }
    }
    setPendingAction(null);
  };
  const clear = async () => {
    setPendingAction("clear");
    const result = await development.clearLxmfDiscovery();
    setCommand(
      result.type === "outcome" && result.outcome === Bindings.LxmfDiscoveryClearOutcome.Cleared
        ? "Discovered contacts cleared. Saved contacts and messages are unchanged."
        : "Discovered contacts could not be cleared. Try again.",
    );
    await directory.refreshDiscovery();
    setPendingAction(null);
  };

  return (
    <>
      <MessagingProfileCard onSavingChange={setSavingProfile} />
      <Card>
        <BodyText muted>
          Announce your messaging name and address on connected networks so others can discover you.
          They may be several hops away.
        </BodyText>
        <Button
          disabled={!directory.nodeRunning || pendingAction !== null || savingProfile}
          onPress={() => void announce()}
        >
          {pendingAction === "announce" ? "Requesting announcement…" : "Announce yourself"}
        </Button>
        {directory.nodeRunning ? null : (
          <BodyText muted>
            Start this phone&apos;s node to announce yourself or discover contacts. Saved contacts
            remain available.
          </BodyText>
        )}
        {command === null ? null : <BodyText>{command}</BodyText>}
      </Card>
      <ActionRow>
        <Button
          accessibilityState={{ selected: mode === "saved" }}
          tone={mode === "saved" ? "primary" : "secondary"}
          onPress={() => setMode("saved")}
        >
          Saved
        </Button>
        <Button
          accessibilityState={{ selected: mode === "discovered" }}
          tone={mode === "discovered" ? "primary" : "secondary"}
          onPress={() => setMode("discovered")}
        >
          Discovered
        </Button>
      </ActionRow>
      <ActionRow>
        <NavigationLink href="/contacts/add">Add contact</NavigationLink>
        <Button
          disabled={directory.pending || directory.discoveryPending}
          onPress={() => void directory.refresh()}
          tone="secondary"
        >
          {directory.pending ? "Loading contacts…" : "Refresh contacts"}
        </Button>
      </ActionRow>
      {directory.failure === null ? null : <FailureCard detail={directory.failure} />}
      {saveFailure === null ? null : <FailureCard detail={saveFailure} />}
      <MutationResult outcome={mutation} />
      {mode === "saved" ? (
        directory.failure === null ? (
          <ContactListResult outcome={directory.outcome} />
        ) : null
      ) : (
        <>
          <BodyText muted>
            Names are supplied by their owners. Discovery is recent activity, not a guarantee that
            someone is reachable now.
          </BodyText>
          {directory.discoveryFailure === null ? null : (
            <FailureCard detail={directory.discoveryFailure} />
          )}
          {directory.peers.length === 0 ? (
            <Card>
              <Badge>
                {directory.discoveryPending ? "Loading discovery" : "No discovered contacts"}
              </Badge>
              <BodyText>
                {directory.discoveryPending
                  ? "Checking recently heard messaging contacts…"
                  : "Ask another person to announce themselves while both nodes are connected."}
              </BodyText>
            </Card>
          ) : (
            directory.peers.map((peer) => {
              const destination = formatContactHash(peer.destination);
              const name = peerLabel(peer.destination, directory.peers, directory.contacts);
              const saved = directory.contacts.find(
                (contact) => formatContactHash(contact.destination) === destination,
              );
              const saveLabel = saved === undefined ? "Save contact" : "Save messaging contact";
              return (
                <Card key={destination}>
                  <CardHeader title={name}>
                    {saved ? <Badge>Saved contact</Badge> : null}
                  </CardHeader>
                  <BodyText muted>
                    {shortDestination(peer.destination)} ·{" "}
                    {lastHeardLabel(peer.lastObservedAgeMillis)}
                  </BodyText>
                  <ActionRow>
                    {saved ? (
                      <NavigationLink
                        href={{ pathname: "/contacts/[destination]", params: { destination } }}
                      >
                        Open contact
                      </NavigationLink>
                    ) : null}
                    {saved?.isMessaging === true ? null : (
                      <Button
                        accessibilityLabel={`${saveLabel} ${name}`}
                        disabled={pendingAction !== null}
                        onPress={() => void save(peer.destination)}
                      >
                        {pendingAction === destination ? "Saving…" : saveLabel}
                      </Button>
                    )}
                    <NavigationLink
                      accessibilityLabel={`Message ${name}`}
                      href={{ pathname: "/inbox/compose", params: { destination } }}
                    >
                      Message
                    </NavigationLink>
                  </ActionRow>
                  <DiscoveryDetails peer={peer} name={name} />
                </Card>
              );
            })
          )}
          <Button
            tone="secondary"
            disabled={
              !directory.nodeRunning || pendingAction !== null || directory.peers.length === 0
            }
            onPress={() => void clear()}
          >
            Clear discovered contacts
          </Button>
          <BodyText muted>
            Discovered contacts expire after a day and reset when the node restarts. Clearing this
            list does not block anyone or remove saved contacts.
          </BodyText>
        </>
      )}
    </>
  );
}

function DiscoveryDetails({
  peer,
  name,
}: {
  readonly peer: LxmfPeerSummary;
  readonly name: string;
}) {
  const { snapshot } = useDevelopmentRuntime();
  const [expanded, setExpanded] = useState(false);
  const ingressId = formatContactHash(peer.sourceInterface);
  const physicalIngress = snapshot?.bluetooth?.peers.find(
    (candidate) => formatContactHash(candidate.interfaceId) === ingressId,
  );
  const ingress =
    snapshot?.localHost?.tag === Bindings.LocalHostState_Tags.Running
      ? snapshot.localHost.inner.host.interfaces.find(
          (candidate) => formatContactHash(candidate.interfaceId) === ingressId,
        )
      : undefined;
  const kind = ingress?.kind === "AutomaticBluetoothLe" ? "Automatic Bluetooth" : ingress?.kind;
  return (
    <>
      <Button
        tone="secondary"
        accessibilityLabel={`${expanded ? "Hide" : "Show"} discovery details for ${name}`}
        accessibilityState={{ expanded }}
        onPress={() => setExpanded((current) => !current)}
      >
        {expanded ? "Hide discovery details" : "Discovery details"}
      </Button>
      {expanded ? (
        <CardSection>
          <KeyValue
            label="Received via"
            value={
              physicalIngress?.name ??
              (physicalIngress === undefined
                ? (ingress?.name ?? kind ?? "Connection no longer listed")
                : "Automatic Bluetooth")
            }
          />
          <KeyValue label="Hop count" value={peer.hops.toString()} />
          <KeyValue label="Connection ID" value={ingressId} />
          <KeyValue label="Identity" value={formatContactHash(peer.identity)} />
          <KeyValue
            label="Announcement"
            value={peer.isPathResponse ? "Path response" : "Announce"}
          />
          <BodyText muted>
            This describes the last announcement received, not the route a future message will take.
          </BodyText>
        </CardSection>
      ) : null}
    </>
  );
}

function announceLabel(outcome: Bindings.AnnounceLxmfOutcome): string {
  switch (outcome) {
    case Bindings.AnnounceLxmfOutcome.Requested:
      return "Announcement requested. Other devices may hear it on connected networks.";
    case Bindings.AnnounceLxmfOutcome.NoUsableConnection:
      return "No usable connection. Open Connections to connect, then announce again.";
    case Bindings.AnnounceLxmfOutcome.LocalNodeStopped:
      return "This phone's node stopped before the announcement was requested.";
    case Bindings.AnnounceLxmfOutcome.Busy:
      return "Another messaging action is in progress. Try again.";
    case Bindings.AnnounceLxmfOutcome.Failed:
      return "The announcement could not be confirmed. Try again.";
  }
}

function ContactListResult({ outcome }: { readonly outcome: ContactListOutcome | null }) {
  if (outcome === null) {
    return (
      <Card>
        <Badge>Loading</Badge>
        <BodyText muted>Loading saved contacts…</BodyText>
      </Card>
    );
  }
  if (outcome.tag === Bindings.ContactListOutcome_Tags.DevelopmentUnavailable) {
    return <FailureCard detail="Contacts could not be loaded. Try again." />;
  }
  if (outcome.tag === Bindings.ContactListOutcome_Tags.DevelopmentResetRequired) {
    return <ResetRequiredCard />;
  }
  if (outcome.inner.contacts.length === 0) {
    return (
      <Card>
        <Badge>No saved contacts</Badge>
        <BodyText>Add a contact, or save a node you discover on the network.</BodyText>
      </Card>
    );
  }
  return (
    <>
      {outcome.inner.contacts.map((contact) => {
        const destination = formatContactHash(contact.destination);
        const href: Href = {
          pathname: "/contacts/[destination]",
          params: { destination },
        };
        return (
          <Card key={destination}>
            <CardHeader
              title={
                contact.alias ?? contact.announcedName ?? shortDestination(contact.destination)
              }
            >
              {contact.pinned ? <Badge>Pinned</Badge> : null}
            </CardHeader>
            <BodyText muted>{destination}</BodyText>
            <NavigationLink href={href}>Open contact</NavigationLink>
          </Card>
        );
      })}
    </>
  );
}

export function ContactDetailScreen({ destination }: { readonly destination: DestinationHash }) {
  const contactRuntime = useContactRuntime();
  const router = useRouter();
  const destinationText = formatContactHash(destination);
  const [lookup, setLookup] = useState<ContactLookupOutcome | null>(null);
  const [contact, setContact] = useState<Contact | null>(null);
  const [alias, setAlias] = useState("");
  const [mutation, setMutation] = useState<ContactMutationOutcome | null>(null);
  const [failure, setFailure] = useState<string | Bindings.NativeStoragePreparationError | null>(
    null,
  );
  const [pending, setPending] = useState(false);

  const load = useCallback(async () => {
    if (contactRuntime.runtime === null) {
      return;
    }
    setPending(true);
    setFailure(null);
    try {
      const next = await contactRuntime.runtime.getContact(destination);
      setLookup(next);
      if (next.tag === Bindings.ContactLookupOutcome_Tags.Found) {
        setContact(next.inner.contact);
        setAlias(next.inner.contact.alias ?? "");
      } else {
        setContact(null);
      }
    } catch (failure) {
      setFailure(
        failure instanceof Bindings.NativeStoragePreparationError
          ? failure
          : "The contact could not be loaded. Try again.",
      );
    } finally {
      setPending(false);
    }
  }, [contactRuntime.runtime, destination]);

  useEffect(() => {
    void load();
  }, [load]);

  const applyMutation = async (
    operation: (runtime: DevelopmentRuntime) => Promise<ContactMutationOutcome>,
  ) => {
    const activeRuntime = contactRuntime.runtime;
    if (activeRuntime === null) {
      setFailure("Contacts are unavailable.");
      return;
    }
    setPending(true);
    setFailure(null);
    setMutation(null);
    try {
      const next = await operation(activeRuntime);
      setMutation(next);
      if (
        next.tag === Bindings.ContactMutationOutcome_Tags.Saved ||
        next.tag === Bindings.ContactMutationOutcome_Tags.Updated ||
        next.tag === Bindings.ContactMutationOutcome_Tags.Existing
      ) {
        setContact(next.inner.contact);
        setAlias(next.inner.contact.alias ?? "");
      } else if (next.tag === Bindings.ContactMutationOutcome_Tags.Deleted) {
        router.replace("/contacts");
      }
    } catch (failure) {
      setFailure(
        failure instanceof Bindings.NativeStoragePreparationError
          ? failure
          : "The contact could not be updated. Try again.",
      );
    } finally {
      setPending(false);
    }
  };

  return (
    <Screen>
      <Badge>Saved destination</Badge>
      <ScreenHeading>{contact?.alias ?? contact?.announcedName ?? "Contact"}</ScreenHeading>
      <KeyValue label="Destination" value={destinationText} />
      {contactRuntime.runtime === null ? (
        <NativeContactsUnavailable platform={contactRuntime.availability.platform} />
      ) : lookup?.tag === Bindings.ContactLookupOutcome_Tags.DevelopmentResetRequired ? (
        <ResetRequiredCard />
      ) : lookup?.tag === Bindings.ContactLookupOutcome_Tags.DevelopmentUnavailable ? (
        <FailureCard detail="The contact could not be loaded. Try again." />
      ) : lookup?.tag === Bindings.ContactLookupOutcome_Tags.NotFound ? (
        <Card>
          <Badge tone="warning">Not found</Badge>
          <BodyText>This destination is not saved in the local directory.</BodyText>
        </Card>
      ) : contact === null ? (
        <Card>
          <Badge>Loading</Badge>
        </Card>
      ) : (
        <>
          {contact.isMessaging ? (
            <NavigationLink
              href={{ pathname: "/inbox/compose", params: { destination: destinationText } }}
            >
              Message
            </NavigationLink>
          ) : (
            <BodyText muted>
              This saved destination has not been identified as a messaging contact.
            </BodyText>
          )}
          <Card>
            <Subheading>Association</Subheading>
            <KeyValue
              label="Identity"
              value={
                contact.identity === undefined ? "Not known" : formatContactHash(contact.identity)
              }
            />
            <KeyValue label="Pinned" value={contact.pinned ? "Yes" : "No"} />
          </Card>
          <Card>
            <Subheading>Private contact name</Subheading>
            <BodyText muted>
              This name is only for you. It does not change the name this contact announces.
            </BodyText>
            <TextField
              label="Name (optional)"
              autoCapitalize="sentences"
              autoCorrect={false}
              onChangeText={setAlias}
              placeholder="e.g. Alex"
              value={alias}
            />
            <Button
              disabled={pending}
              onPress={() =>
                void applyMutation((runtime) => runtime.setContactAlias(destination, alias))
              }
            >
              Save name
            </Button>
          </Card>
          <Button
            disabled={pending}
            onPress={() =>
              void applyMutation((runtime) =>
                runtime.setContactPinned(destination, !contact.pinned),
              )
            }
            tone="secondary"
          >
            {contact.pinned ? "Unpin contact" : "Pin contact"}
          </Button>
          <Button
            disabled={pending}
            onPress={() => void applyMutation((runtime) => runtime.deleteContact(destination))}
            tone="destructive"
          >
            Delete contact
          </Button>
        </>
      )}
      {failure === null ? null : <FailureCard detail={failure} />}
      <MutationResult outcome={mutation} />
      <NavigationLink href="/contacts" direction="back">
        Back to Contacts
      </NavigationLink>
    </Screen>
  );
}

export function AddContactScreen() {
  const contactRuntime = useContactRuntime();
  const router = useRouter();
  const [destinationText, setDestinationText] = useState("");
  const [identityText, setIdentityText] = useState("");
  const [alias, setAlias] = useState("");
  const [outcome, setOutcome] = useState<ContactMutationOutcome | null>(null);
  const [failure, setFailure] = useState<string | Bindings.NativeStoragePreparationError | null>(
    null,
  );
  const [pending, setPending] = useState(false);

  const create = async () => {
    const destination = parseDestinationHash(destinationText);
    if (destination === null) {
      setFailure("Destination must be exactly 32 hexadecimal characters.");
      return;
    }
    const identityInput = identityText.trim();
    const identity = identityInput.length === 0 ? null : parseIdentityHash(identityInput);
    if (identityInput.length !== 0 && identity === null) {
      setFailure("Identity must be empty or exactly 32 hexadecimal characters.");
      return;
    }
    if (contactRuntime.runtime === null) {
      return;
    }
    setPending(true);
    setFailure(null);
    try {
      const next = await contactRuntime.runtime.createManualContact(
        destination,
        identity ?? undefined,
        alias || undefined,
      );
      setOutcome(next);
      if (
        next.tag === Bindings.ContactMutationOutcome_Tags.Saved ||
        next.tag === Bindings.ContactMutationOutcome_Tags.AlreadyExists
      ) {
        const href: Href = {
          pathname: "/contacts/[destination]",
          params: { destination: formatContactHash(next.inner.contact.destination) },
        };
        router.replace(href);
      }
    } catch (failure) {
      setFailure(
        failure instanceof Bindings.NativeStoragePreparationError
          ? failure
          : "The contact could not be saved. Try again.",
      );
    } finally {
      setPending(false);
    }
  };

  return (
    <Screen>
      <Badge>New contact</Badge>
      <ScreenHeading>Add contact</ScreenHeading>
      <BodyText>
        Enter a destination and, optionally, an identity. A contact needs an identity before it can
        be pinned.
      </BodyText>
      {contactRuntime.runtime === null ? (
        <NativeContactsUnavailable platform={contactRuntime.availability.platform} />
      ) : (
        <Card>
          <TextField
            label="Destination"
            autoCapitalize="none"
            autoCorrect={false}
            onChangeText={setDestinationText}
            placeholder="32 hexadecimal characters"
            value={destinationText}
          />
          <TextField
            label="Identity (optional)"
            autoCapitalize="none"
            autoCorrect={false}
            onChangeText={setIdentityText}
            placeholder="Optional 32-character identity hash"
            value={identityText}
          />
          <TextField
            label="Name (optional)"
            autoCapitalize="sentences"
            autoCorrect={false}
            onChangeText={setAlias}
            placeholder="e.g. Alex"
            value={alias}
          />
          <Button disabled={pending} onPress={() => void create()}>
            {pending ? "Saving…" : "Save contact"}
          </Button>
        </Card>
      )}
      {failure === null ? null : <FailureCard detail={failure} />}
      <MutationResult outcome={outcome} />
      <BodyText muted>You can also save messaging contacts from Contacts &gt; Discovered.</BodyText>
      <NavigationLink href="/contacts" direction="back">
        Back to Contacts
      </NavigationLink>
    </Screen>
  );
}

function MutationResult({ outcome }: { readonly outcome: ContactMutationOutcome | null }) {
  if (outcome === null) {
    return null;
  }
  if (outcome.tag === Bindings.ContactMutationOutcome_Tags.DevelopmentUnavailable) {
    return <FailureCard detail="The contact could not be updated. Try again." />;
  }
  if (outcome.tag === Bindings.ContactMutationOutcome_Tags.DevelopmentResetRequired) {
    return <ResetRequiredCard />;
  }
  const messages: Record<
    Exclude<
      ContactMutationOutcome["tag"],
      | Bindings.ContactMutationOutcome_Tags.DevelopmentUnavailable
      | Bindings.ContactMutationOutcome_Tags.DevelopmentResetRequired
    >,
    string
  > = {
    [Bindings.ContactMutationOutcome_Tags.Saved]: "Contact saved.",
    [Bindings.ContactMutationOutcome_Tags.Updated]: "Contact updated.",
    [Bindings.ContactMutationOutcome_Tags.Deleted]: "Contact deleted.",
    [Bindings.ContactMutationOutcome_Tags.Existing]: "This verified destination was already saved.",
    [Bindings.ContactMutationOutcome_Tags.AlreadyExists]: "This destination is already saved.",
    [Bindings.ContactMutationOutcome_Tags.NotFound]: "The contact no longer exists.",
    [Bindings.ContactMutationOutcome_Tags.LocalNodeStopped]: "This device's node is not running.",
    [Bindings.ContactMutationOutcome_Tags.NotObserved]:
      "This destination is no longer visible on the network.",
    [Bindings.ContactMutationOutcome_Tags.IdentityConflict]:
      "The saved identity differs from the verified network identity.",
    [Bindings.ContactMutationOutcome_Tags.MissingIdentity]:
      "Add or discover an identity before pinning this contact.",
  };
  return (
    <Card>
      <Badge
        tone={
          outcome.tag === Bindings.ContactMutationOutcome_Tags.IdentityConflict
            ? "warning"
            : "neutral"
        }
      >
        {messages[outcome.tag]}
      </Badge>
      {outcome.tag === Bindings.ContactMutationOutcome_Tags.IdentityConflict ? (
        <>
          <KeyValue label="Saved identity" value={formatContactHash(outcome.inner.existing)} />
          <KeyValue label="Observed identity" value={formatContactHash(outcome.inner.attempted)} />
        </>
      ) : null}
    </Card>
  );
}

function NativeContactsUnavailable({ platform }: { readonly platform: string }) {
  return (
    <Card>
      <Badge tone="warning">Contacts unavailable</Badge>
      <BodyText>Contacts are not available on {platform} yet.</BodyText>
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
      <Badge tone="warning">Contacts unavailable</Badge>
      <BodyText>{detail}</BodyText>
    </Card>
  );
}

function ResetRequiredCard() {
  return (
    <Card>
      <Badge tone="warning">App reset required</Badge>
      <BodyText>This app&apos;s data needs to be reset before contacts can be used.</BodyText>
      <NavigationLink href="/recovery">Open recovery</NavigationLink>
    </Card>
  );
}
