import type {
  Contact,
  ContactListOutcome,
  ContactLookupOutcome,
  ContactMutationOutcome,
  DevelopmentRuntime,
} from "@prns-internal/expo";
import type { Href } from "expo-router";
import { useRouter } from "expo-router";
import type { DestinationHash } from "personal-rns/contract";
import { useCallback, useEffect, useState } from "react";
import { StyleSheet, TextInput } from "react-native";

import { useContactRuntime } from "@/native/contact-runtime-context";
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
import { formatContactHash, parseDestinationHash, parseIdentityHash } from "./format";

export function ContactsScreen() {
  const contactRuntime = useContactRuntime();
  const [outcome, setOutcome] = useState<ContactListOutcome | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const load = useCallback(async () => {
    if (contactRuntime.runtime === null) {
      return;
    }
    setPending(true);
    setFailure(null);
    try {
      setOutcome(await contactRuntime.runtime.listContacts());
    } catch (error) {
      setFailure(formatThrown(error));
    } finally {
      setPending(false);
    }
  }, [contactRuntime.runtime]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <Screen>
      <Badge>Local directory</Badge>
      <ScreenHeading>Contacts</ScreenHeading>
      <BodyText>
        Saved destinations are read directly from the application-owned Rust directory. Live
        observations remain in the local Host until you deliberately save one.
      </BodyText>
      {contactRuntime.runtime === null ? (
        <NativeContactsUnavailable platform={contactRuntime.availability.platform} />
      ) : (
        <>
          <Button disabled={pending} onPress={() => void load()} tone="secondary">
            {pending ? "Loading contacts…" : "Refresh contacts"}
          </Button>
          {failure === null ? null : <FailureCard detail={failure} />}
          <ContactListResult outcome={outcome} />
          <NavigationLink href="/contacts/add">Add a manual contact</NavigationLink>
        </>
      )}
    </Screen>
  );
}

function ContactListResult({ outcome }: { readonly outcome: ContactListOutcome | null }) {
  if (outcome === null) {
    return (
      <Card>
        <Badge>Loading</Badge>
        <BodyText muted>Opening the persisted contact table.</BodyText>
      </Card>
    );
  }
  if (outcome.type === "developmentUnavailable") {
    return <FailureCard detail={outcome.detail} />;
  }
  if (outcome.type === "developmentResetRequired") {
    return <ResetRequiredCard reason={outcome.reason} />;
  }
  if (outcome.contacts.length === 0) {
    return (
      <Card>
        <Badge>No saved contacts</Badge>
        <BodyText>
          Add a manual destination here, or save an authenticated observation while the local node
          is running.
        </BodyText>
      </Card>
    );
  }
  return (
    <>
      {outcome.contacts.map((contact) => {
        const destination = formatContactHash(contact.destination);
        const href: Href = {
          pathname: "/contacts/[destination]",
          params: { destination },
        };
        return (
          <Card key={destination}>
            <Subheading>{contact.alias ?? destination}</Subheading>
            <Badge>{contact.pinned ? "Pinned" : "Saved"}</Badge>
            <KeyValue label="Destination" value={destination} />
            <KeyValue
              label="Identity"
              value={contact.identity === null ? "Not known" : formatContactHash(contact.identity)}
            />
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
  const [failure, setFailure] = useState<string | null>(null);
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
      if (next.type === "found") {
        setContact(next.contact);
        setAlias(next.contact.alias ?? "");
      } else {
        setContact(null);
      }
    } catch (error) {
      setFailure(formatThrown(error));
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
      setFailure("The native contact runtime is unavailable.");
      return;
    }
    setPending(true);
    setFailure(null);
    setMutation(null);
    try {
      const next = await operation(activeRuntime);
      setMutation(next);
      if (next.type === "saved" || next.type === "updated" || next.type === "existing") {
        setContact(next.contact);
        setAlias(next.contact.alias ?? "");
      } else if (next.type === "deleted") {
        router.replace("/contacts");
      }
    } catch (error) {
      setFailure(formatThrown(error));
    } finally {
      setPending(false);
    }
  };

  return (
    <Screen>
      <Badge>Saved destination</Badge>
      <ScreenHeading>{contact?.alias ?? "Contact"}</ScreenHeading>
      <KeyValue label="Destination" value={destinationText} />
      {contactRuntime.runtime === null ? (
        <NativeContactsUnavailable platform={contactRuntime.availability.platform} />
      ) : lookup?.type === "developmentResetRequired" ? (
        <ResetRequiredCard reason={lookup.reason} />
      ) : lookup?.type === "developmentUnavailable" ? (
        <FailureCard detail={lookup.detail} />
      ) : lookup?.type === "notFound" ? (
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
          <Card>
            <Subheading>Association</Subheading>
            <KeyValue
              label="Identity"
              value={contact.identity === null ? "Not known" : formatContactHash(contact.identity)}
            />
            <KeyValue label="Pinned" value={contact.pinned ? "Yes" : "No"} />
          </Card>
          <Card>
            <Subheading>Alias</Subheading>
            <ContactField
              accessibilityLabel="Contact alias"
              onChangeText={setAlias}
              placeholder="Optional alias"
              value={alias}
            />
            <Button
              disabled={pending}
              onPress={() =>
                void applyMutation((runtime) => runtime.setContactAlias(destination, alias))
              }
            >
              Save alias
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
      <NavigationLink href="/contacts">Back to Contacts</NavigationLink>
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
  const [failure, setFailure] = useState<string | null>(null);
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
      const next = await contactRuntime.runtime.createManualContact(destination, identity, alias);
      setOutcome(next);
      if (next.type === "saved" || next.type === "alreadyExists") {
        const href: Href = {
          pathname: "/contacts/[destination]",
          params: { destination: formatContactHash(next.contact.destination) },
        };
        router.replace(href);
      }
    } catch (error) {
      setFailure(formatThrown(error));
    } finally {
      setPending(false);
    }
  };

  return (
    <Screen>
      <Badge>Manual entry</Badge>
      <ScreenHeading>Add contact</ScreenHeading>
      <BodyText>
        Enter a destination directly. An identity is optional, but Rust will not allow this contact
        to be pinned until one is known.
      </BodyText>
      {contactRuntime.runtime === null ? (
        <NativeContactsUnavailable platform={contactRuntime.availability.platform} />
      ) : (
        <Card>
          <ContactField
            accessibilityLabel="Destination hash"
            autoCapitalize="none"
            onChangeText={setDestinationText}
            placeholder="32 hexadecimal characters"
            value={destinationText}
          />
          <ContactField
            accessibilityLabel="Identity hash"
            autoCapitalize="none"
            onChangeText={setIdentityText}
            placeholder="Optional 32-character identity hash"
            value={identityText}
          />
          <ContactField
            accessibilityLabel="Contact alias"
            onChangeText={setAlias}
            placeholder="Optional alias"
            value={alias}
          />
          <Button disabled={pending} onPress={() => void create()}>
            {pending ? "Saving…" : "Save manual contact"}
          </Button>
        </Card>
      )}
      {failure === null ? null : <FailureCard detail={failure} />}
      <MutationResult outcome={outcome} />
      <BodyText muted>
        To save a network-authenticated association, use Save as contact beside an observation on
        the running local-node screen.
      </BodyText>
      <NavigationLink href="/contacts">Back to Contacts</NavigationLink>
    </Screen>
  );
}

function ContactField({
  accessibilityLabel,
  autoCapitalize = "sentences",
  onChangeText,
  placeholder,
  value,
}: {
  readonly accessibilityLabel: string;
  readonly autoCapitalize?: "none" | "sentences";
  readonly onChangeText: (value: string) => void;
  readonly placeholder: string;
  readonly value: string;
}) {
  const palette = useAppPalette();
  return (
    <TextInput
      accessibilityLabel={accessibilityLabel}
      autoCapitalize={autoCapitalize}
      autoCorrect={false}
      onChangeText={onChangeText}
      placeholder={placeholder}
      placeholderTextColor={palette.textMuted}
      style={[
        styles.input,
        { backgroundColor: palette.background, borderColor: palette.border, color: palette.text },
      ]}
      value={value}
    />
  );
}

function MutationResult({ outcome }: { readonly outcome: ContactMutationOutcome | null }) {
  if (outcome === null) {
    return null;
  }
  if (outcome.type === "developmentUnavailable") {
    return <FailureCard detail={outcome.detail} />;
  }
  if (outcome.type === "developmentResetRequired") {
    return <ResetRequiredCard reason={outcome.reason} />;
  }
  const messages: Record<
    Exclude<ContactMutationOutcome["type"], "developmentUnavailable" | "developmentResetRequired">,
    string
  > = {
    saved: "Contact saved.",
    updated: "Contact updated.",
    deleted: "Contact deleted.",
    existing: "This authenticated association was already saved.",
    alreadyExists: "This destination is already saved.",
    notFound: "The contact no longer exists.",
    localNodeStopped: "The local node is not running.",
    notObserved: "The running node has no authenticated identity for this destination.",
    identityConflict: "The saved identity differs from the authenticated observation.",
    missingIdentity: "Add or observe an identity before pinning this contact.",
  };
  return (
    <Card>
      <Badge tone={outcome.type === "identityConflict" ? "warning" : "neutral"}>
        {messages[outcome.type]}
      </Badge>
      {outcome.type === "identityConflict" ? (
        <>
          <KeyValue label="Saved identity" value={formatContactHash(outcome.existing)} />
          <KeyValue label="Observed identity" value={formatContactHash(outcome.attempted)} />
        </>
      ) : null}
    </Card>
  );
}

function NativeContactsUnavailable({ platform }: { readonly platform: string }) {
  return (
    <Card>
      <Badge tone="warning">Native directory unavailable</Badge>
      <BodyText>
        The {platform} build has no application-owned native contact provider. No contacts are being
        simulated or stored by this screen.
      </BodyText>
    </Card>
  );
}

function FailureCard({ detail }: { readonly detail: string }) {
  return (
    <Card>
      <Badge tone="warning">Directory unavailable</Badge>
      <BodyText>{detail}</BodyText>
    </Card>
  );
}

function ResetRequiredCard({ reason }: { readonly reason: string }) {
  return (
    <Card>
      <Badge tone="warning">Development reset required</Badge>
      <BodyText>{reason}</BodyText>
      <NavigationLink href="/recovery">Open development recovery</NavigationLink>
    </Card>
  );
}

function formatThrown(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const styles = StyleSheet.create({
  input: {
    borderRadius: radius.sm,
    borderWidth: 1,
    fontSize: 16,
    minHeight: 48,
    paddingHorizontal: space.md,
    paddingVertical: space.sm,
  },
});
