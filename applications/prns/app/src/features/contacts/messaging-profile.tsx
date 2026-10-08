import * as Bindings from "@prns-internal/expo";
import type { LocalMessagingProfile, LocalMessagingProfileOutcome } from "@prns-internal/expo";
import { useCallback, useEffect, useRef, useState } from "react";
import { Share } from "react-native";

import {
  useDevelopmentRuntime,
  type RuntimeCommandResult,
} from "@/native/development-runtime-context";
import { StoragePreparationFailure } from "@/native/storage-preparation-failure";
import { NavigationLink } from "@/ui/navigation-link";
import {
  ActionRow,
  BodyText,
  Button,
  Card,
  CardHeader,
  CardSection,
  KeyValue,
} from "@/ui/primitives";
import { TextField } from "@/ui/text-field";
import { formatContactHash } from "./format";
import type { DirectoryFailure } from "./messaging-directory";

export function MessagingProfileCard({
  onSavingChange,
}: {
  readonly onSavingChange?: (saving: boolean) => void;
}) {
  const { readMessagingProfile, setMessagingName } = useDevelopmentRuntime();
  const [profile, setProfile] = useState<LocalMessagingProfile | null>(null);
  const [name, setName] = useState("");
  const [editing, setEditing] = useState(false);
  const [showAddress, setShowAddress] = useState(false);
  const [pending, setPending] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [failure, setFailure] = useState<DirectoryFailure | null>(null);
  const [resetRequired, setResetRequired] = useState(false);
  const requestId = useRef(0);

  const accept = useCallback(
    (result: RuntimeCommandResult<LocalMessagingProfileOutcome>, saving: boolean) => {
      setFailure(null);
      setResetRequired(false);
      if (result.type === "operationFailure") {
        setFailure(
          result.storagePreparation === undefined
            ? "The messaging name could not be confirmed. Refresh to check it, then try again."
            : new Bindings.NativeStoragePreparationError(result.storagePreparation),
        );
        return;
      }
      const outcome = result.outcome;
      switch (outcome.tag) {
        case Bindings.LocalMessagingProfileOutcome_Tags.Ready:
        case Bindings.LocalMessagingProfileOutcome_Tags.SavedButNotApplied:
          setProfile(outcome.inner.profile);
          setName(outcome.inner.profile.displayName);
          if (saving) setEditing(false);
          setFeedback(
            outcome.tag === Bindings.LocalMessagingProfileOutcome_Tags.SavedButNotApplied
              ? "Name saved. It could not be applied to the running node yet. Try saving it again before announcing."
              : saving
                ? "Messaging name saved. Announce yourself when you want to share it."
                : null,
          );
          return;
        case Bindings.LocalMessagingProfileOutcome_Tags.InvalidInput:
          setFailure("Choose a shorter name without line breaks or special control characters.");
          return;
        case Bindings.LocalMessagingProfileOutcome_Tags.Busy:
          setFailure("Another messaging setting is being updated. Try again.");
          return;
        case Bindings.LocalMessagingProfileOutcome_Tags.Unavailable:
          setFailure(
            "The messaging name could not be confirmed. Refresh to check it, then try again.",
          );
          return;
        case Bindings.LocalMessagingProfileOutcome_Tags.DevelopmentResetRequired:
          setResetRequired(true);
      }
    },
    [],
  );

  const refresh = useCallback(async () => {
    const request = ++requestId.current;
    setPending(true);
    const result = await readMessagingProfile();
    if (request !== requestId.current) return;
    accept(result, false);
    setPending(false);
  }, [accept, readMessagingProfile]);

  useEffect(() => {
    void refresh();
    return () => {
      requestId.current += 1;
      onSavingChange?.(false);
    };
  }, [refresh, onSavingChange]);

  const save = async () => {
    const request = ++requestId.current;
    setPending(true);
    onSavingChange?.(true);
    setFeedback(null);
    const result = await setMessagingName(name);
    if (request !== requestId.current) return;
    accept(result, true);
    setPending(false);
    onSavingChange?.(false);
  };

  const share = async () => {
    if (profile?.destination === undefined) return;
    try {
      await Share.share({ message: formatContactHash(profile.destination) });
    } catch {
      setFeedback("The address could not be shared. Try again.");
    }
  };

  return (
    <Card>
      <CardHeader title="Your messaging name" />
      <BodyText>
        {profile?.displayName ??
          (failure !== null || resetRequired
            ? "Messaging name unavailable"
            : "Loading messaging name…")}
      </BodyText>
      <ActionRow>
        <Button
          disabled={pending || profile === null}
          tone="secondary"
          accessibilityState={{ expanded: editing }}
          onPress={() => {
            setName(profile?.displayName ?? "");
            setEditing((current) => !current);
          }}
        >
          {editing ? "Cancel name edit" : "Edit messaging name"}
        </Button>
        <Button
          disabled={profile === null}
          tone="secondary"
          accessibilityState={{ expanded: showAddress }}
          onPress={() => setShowAddress((current) => !current)}
        >
          {showAddress ? "Hide my address" : "My address"}
        </Button>
      </ActionRow>
      {editing ? (
        <CardSection>
          <BodyText muted>
            This name accompanies your messaging announcements. It does not change your identity or
            anyone&apos;s private name for you.
          </BodyText>
          <TextField
            label="Messaging name"
            value={name}
            onChangeText={setName}
            editable={!pending}
            autoCapitalize="words"
          />
          <Button disabled={pending} onPress={() => void save()}>
            {pending ? "Saving name…" : "Save messaging name"}
          </Button>
        </CardSection>
      ) : null}
      {showAddress ? (
        <CardSection>
          {profile?.destination === undefined ? (
            <BodyText>Create an identity before sharing your messaging address.</BodyText>
          ) : (
            <>
              <KeyValue label="Messaging address" value={formatContactHash(profile.destination)} />
              <BodyText muted>
                Share this address using another app. This does not announce you on the network.
              </BodyText>
              <Button tone="secondary" onPress={() => void share()}>
                Share address
              </Button>
            </>
          )}
        </CardSection>
      ) : null}
      {feedback === null ? null : <BodyText>{feedback}</BodyText>}
      {failure instanceof Bindings.NativeStoragePreparationError ? (
        <StoragePreparationFailure outcome={failure.outcome} />
      ) : failure === null ? null : (
        <BodyText>{failure}</BodyText>
      )}
      {resetRequired ? (
        <>
          <BodyText>App data needs to be reset before the messaging profile can be used.</BodyText>
          <NavigationLink href="/recovery">Open recovery</NavigationLink>
        </>
      ) : null}
      {failure === null && !resetRequired ? null : (
        <Button disabled={pending} tone="secondary" onPress={() => void refresh()}>
          Refresh messaging name
        </Button>
      )}
    </Card>
  );
}
