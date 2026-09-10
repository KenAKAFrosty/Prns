import * as Bindings from "@prns-internal/expo";
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

import { StoragePreparationFailure } from "@/native/storage-preparation-failure";
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
import { TextField } from "@/ui/text-field";
import { formatContactHash, parseDestinationHash, parseIdentityHash } from "./format";

export function ContactsScreen() {
  const contactRuntime = useContactRuntime();
  const [outcome, setOutcome] = useState<ContactListOutcome | null>(null);
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
      setOutcome(await contactRuntime.runtime.listContacts());
    } catch (failure) {
      setFailure(
        failure instanceof Bindings.NativeStoragePreparationError
          ? failure
          : "Contacts could not be loaded. Try again.",
      );
    } finally {
      setPending(false);
    }
  }, [contactRuntime.runtime]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <Screen>
      <Badge>On this device</Badge>
      <ScreenHeading>Contacts</ScreenHeading>
      <BodyText>
        Contacts stay on this device. Nodes discovered on the network are added only when you choose
        to save them.
      </BodyText>
      {contactRuntime.runtime === null ? (
        <NativeContactsUnavailable platform={contactRuntime.availability.platform} />
      ) : (
        <>
          <Button disabled={pending} onPress={() => void load()} tone="secondary">
            {pending ? "Loading contacts…" : "Refresh contacts"}
          </Button>
          {failure === null ? null : <FailureCard detail={failure} />}
          {failure === null ? <ContactListResult outcome={outcome} /> : null}
          <NavigationLink href="/contacts/add">Add contact</NavigationLink>
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
            <Subheading>{contact.alias ?? destination}</Subheading>
            <Badge>{contact.pinned ? "Pinned" : "Saved"}</Badge>
            <KeyValue label="Destination" value={destination} />
            <KeyValue
              label="Identity"
              value={
                contact.identity === undefined ? "Not known" : formatContactHash(contact.identity)
              }
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
      <ScreenHeading>{contact?.alias ?? "Contact"}</ScreenHeading>
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
            <Subheading>Contact name</Subheading>
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
      <BodyText muted>
        You can also save a verified destination from Nodes &gt; This device.
      </BodyText>
      <NavigationLink href="/contacts">Back to Contacts</NavigationLink>
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
