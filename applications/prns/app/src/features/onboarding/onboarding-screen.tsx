import type { IdentityCreationOutcome } from "@prns-internal/expo";
import * as DocumentPicker from "expo-document-picker";
import { File } from "expo-file-system";
import { useRouter } from "expo-router";
import { useState } from "react";

import { formatBytes } from "@/features/nodes/format";
import { runtimeProvider } from "@/native/runtime-provider";
import { screenById } from "@/navigation/catalog";
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
import { NotYetImplementedScreen } from "../placeholder-screen";

export type OnboardingStep =
  | "welcome"
  | "create"
  | "import"
  | "retention"
  | "provision"
  | "interfaces"
  | "pairing"
  | "complete";

const implementedSteps: readonly OnboardingStep[] = ["welcome", "create", "import"];

export function isOnboardingStep(value: string): value is OnboardingStep {
  return [
    "welcome",
    "create",
    "import",
    "retention",
    "provision",
    "interfaces",
    "pairing",
    "complete",
  ].includes(value);
}

export function OnboardingScreen({ step }: { readonly step: OnboardingStep }) {
  if (!implementedSteps.includes(step)) {
    return <NotYetImplementedScreen entry={screenById("installation.onboarding")} />;
  }
  if (step === "welcome") {
    return <Welcome />;
  }
  if (step === "create") {
    return <CreateIdentity />;
  }
  return <ImportIdentity />;
}

function Welcome() {
  const router = useRouter();
  return (
    <Screen>
      <Badge>Local development node</Badge>
      <ScreenHeading>Welcome to prns</ScreenHeading>
      <BodyText>
        Create a new Reticulum identity or import one raw 64-byte identity credential. The identity
        stays in this app&apos;s disposable development storage and is required before the local
        node starts.
      </BodyText>
      <CardStack>
        <Button onPress={() => router.push("/onboarding/create")}>Create a new identity</Button>
        <Button tone="secondary" onPress={() => router.push("/onboarding/import")}>
          Import an existing identity
        </Button>
        <Button tone="secondary" onPress={() => router.push("/recovery")}>
          Inspect or reset development data
        </Button>
      </CardStack>
      <BodyText muted>
        This development flow does not provide export, backup, recovery, or secure-custody claims.
      </BodyText>
    </Screen>
  );
}

function CreateIdentity() {
  const router = useRouter();
  const [pending, setPending] = useState(false);
  const [outcome, setOutcome] = useState<IdentityCreationOutcome | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const unavailable = runtimeProvider.availability.type !== "available";

  const create = async () => {
    if (!("runtime" in runtimeProvider)) {
      return;
    }
    setPending(true);
    setFailure(null);
    try {
      setOutcome(await runtimeProvider.runtime.createGeneratedIdentity());
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    } finally {
      setPending(false);
    }
  };

  return (
    <Screen>
      <Badge>Create identity</Badge>
      <ScreenHeading>Create a new identity</ScreenHeading>
      <BodyText>
        Rust generates the private material from the operating system&apos;s cryptographic random
        source and stores it only after this explicit action.
      </BodyText>
      {unavailable ? <PlatformUnavailable /> : null}
      <CreationResult outcome={outcome} failure={failure} />
      <CardStack>
        {outcome?.type === "created" || outcome?.type === "alreadyExists" ? (
          <Button onPress={() => router.replace("/nodes/local")}>Continue to local node</Button>
        ) : (
          <Button disabled={pending || unavailable} onPress={() => void create()}>
            {pending ? "Creating…" : "Create identity"}
          </Button>
        )}
        <Button tone="secondary" onPress={() => router.back()}>
          Back
        </Button>
      </CardStack>
    </Screen>
  );
}

function ImportIdentity() {
  const router = useRouter();
  const [identity, setIdentity] = useState<Uint8Array | null>(null);
  const [previewHash, setPreviewHash] = useState<Uint8Array | null>(null);
  const [pending, setPending] = useState<"pick" | "import" | null>(null);
  const [outcome, setOutcome] = useState<IdentityCreationOutcome | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const unavailable = runtimeProvider.availability.type !== "available";

  const pick = async () => {
    if (!("runtime" in runtimeProvider)) {
      return;
    }
    let cachedCredential: File | null = null;
    setPending("pick");
    setFailure(null);
    setOutcome(null);
    try {
      const selected = await DocumentPicker.getDocumentAsync({
        copyToCacheDirectory: true,
        multiple: false,
      });
      if (selected.canceled) {
        return;
      }
      const asset = selected.assets[0];
      if (asset === undefined) {
        setFailure("The document picker returned no credential.");
        return;
      }
      cachedCredential = new File(asset.uri);
      const bytes = await cachedCredential.bytes();
      const preview = await runtimeProvider.runtime.previewIdentityImport(bytes);
      if (preview.type === "invalidLength") {
        setIdentity(null);
        setPreviewHash(null);
        setFailure(
          `The selected credential is ${bytes.byteLength} bytes; exactly 64 are required.`,
        );
        return;
      }
      setIdentity(bytes);
      setPreviewHash(preview.identityHash);
    } catch (error) {
      setIdentity(null);
      setPreviewHash(null);
      setFailure(error instanceof Error ? error.message : String(error));
    } finally {
      if (cachedCredential !== null) {
        try {
          cachedCredential.delete();
        } catch (error) {
          setIdentity(null);
          setPreviewHash(null);
          const detail = error instanceof Error ? error.message : String(error);
          setFailure(`Could not remove the temporary credential copy: ${detail}`);
        }
      }
      setPending(null);
    }
  };

  const confirm = async () => {
    if (!("runtime" in runtimeProvider) || identity === null) {
      return;
    }
    setPending("import");
    setFailure(null);
    try {
      const result = await runtimeProvider.runtime.createImportedIdentity(identity);
      setOutcome(result);
      if (result.type === "created") {
        setIdentity(null);
      }
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    } finally {
      setPending(null);
    }
  };

  return (
    <Screen>
      <Badge>Import identity</Badge>
      <ScreenHeading>Import an existing identity</ScreenHeading>
      <BodyText>
        Select a raw 64-byte Reticulum private identity credential. TypeScript keeps the selected
        bytes only in this screen&apos;s memory; Rust derives the preview hash and validates the
        bytes again before storing them.
      </BodyText>
      {unavailable ? <PlatformUnavailable /> : null}
      {previewHash === null ? null : (
        <Card>
          <Subheading>Confirm identity</Subheading>
          <KeyValue label="Identity hash" value={formatBytes(previewHash)} />
        </Card>
      )}
      <CreationResult outcome={outcome} failure={failure} />
      <CardStack>
        {outcome?.type === "created" || outcome?.type === "alreadyExists" ? (
          <Button onPress={() => router.replace("/nodes/local")}>Continue to local node</Button>
        ) : (
          <>
            <Button disabled={pending !== null || unavailable} onPress={() => void pick()}>
              {pending === "pick" ? "Reading…" : "Choose raw credential"}
            </Button>
            <Button
              disabled={identity === null || pending !== null}
              onPress={() => void confirm()}
              tone="secondary"
            >
              {pending === "import" ? "Importing…" : "Confirm and import"}
            </Button>
          </>
        )}
        <Button tone="secondary" onPress={() => router.back()}>
          Back
        </Button>
      </CardStack>
    </Screen>
  );
}

function CreationResult({
  failure,
  outcome,
}: {
  readonly failure: string | null;
  readonly outcome: IdentityCreationOutcome | null;
}) {
  if (failure !== null) {
    return (
      <Card>
        <Badge tone="warning">Native operation failed</Badge>
        <BodyText>{failure}</BodyText>
      </Card>
    );
  }
  if (outcome === null) {
    return null;
  }
  switch (outcome.type) {
    case "created":
      return (
        <Card>
          <Badge>Identity stored</Badge>
          <KeyValue label="Identity hash" value={formatBytes(outcome.identityHash)} />
        </Card>
      );
    case "alreadyExists":
      return (
        <Card>
          <Badge>A primary identity already exists</Badge>
          <BodyText>The existing identity remains the sole source of truth.</BodyText>
        </Card>
      );
    case "invalidLength":
      return (
        <Card>
          <Badge tone="warning">Invalid credential length</Badge>
          <BodyText>Select a raw credential containing exactly 64 bytes.</BodyText>
        </Card>
      );
    case "unavailable":
      return (
        <Card>
          <Badge tone="warning">Identity storage unavailable</Badge>
          <BodyText>{outcome.detail}</BodyText>
        </Card>
      );
    case "developmentResetRequired":
      return (
        <Card>
          <Badge tone="warning">Development reset required</Badge>
          <BodyText>{outcome.reason}</BodyText>
        </Card>
      );
  }
}

function PlatformUnavailable() {
  return (
    <Card>
      <Badge tone="warning">iOS development build required</Badge>
      <BodyText>
        Identity creation and import are not implemented for this platform. No synthetic identity
        will be substituted.
      </BodyText>
    </Card>
  );
}
