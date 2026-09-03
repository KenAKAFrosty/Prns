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
      <Badge>Get started</Badge>
      <ScreenHeading>Welcome to prns</ScreenHeading>
      <BodyText>
        Create a new Reticulum identity or import one from a file. Your identity stays on this
        device and lets prns connect to the network.
      </BodyText>
      <CardStack>
        <Button onPress={() => router.push("/onboarding/create")}>Create a new identity</Button>
        <Button tone="secondary" onPress={() => router.push("/onboarding/import")}>
          Import an existing identity
        </Button>
        <Button tone="secondary" onPress={() => router.push("/recovery")}>
          Recovery options
        </Button>
      </CardStack>
      <BodyText muted>Identity export and backup are not available in this preview.</BodyText>
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
    } catch {
      setFailure("Identity could not be created. Try again.");
    } finally {
      setPending(false);
    }
  };

  return (
    <Screen>
      <Badge>Create identity</Badge>
      <ScreenHeading>Create a new identity</ScreenHeading>
      <BodyText>
        Your identity will be generated securely on this device and saved when you continue.
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
    } catch {
      setIdentity(null);
      setPreviewHash(null);
      setFailure("The identity file could not be read.");
    } finally {
      if (cachedCredential !== null) {
        try {
          cachedCredential.delete();
        } catch {
          setIdentity(null);
          setPreviewHash(null);
          setFailure("The temporary identity file could not be removed. Choose the file again.");
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
    } catch {
      setFailure("Identity could not be imported. Try again.");
    } finally {
      setPending(null);
    }
  };

  return (
    <Screen>
      <Badge>Import identity</Badge>
      <ScreenHeading>Import an existing identity</ScreenHeading>
      <BodyText>
        Choose a Reticulum identity file, then check the identity hash before importing it.
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
              {pending === "pick" ? "Reading…" : "Choose identity file"}
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
        <Badge tone="warning">Identity action failed</Badge>
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
          <Badge>This app already has an identity</Badge>
          <BodyText>The existing identity will continue to be used.</BodyText>
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
          <BodyText>Identity storage is unavailable. Try again.</BodyText>
        </Card>
      );
    case "developmentResetRequired":
      return (
        <Card>
          <Badge tone="warning">App reset required</Badge>
          <BodyText>Reset app data before continuing identity setup.</BodyText>
        </Card>
      );
  }
}

function PlatformUnavailable() {
  return (
    <Card>
      <Badge tone="warning">Identity setup unavailable</Badge>
      <BodyText>Identity setup is not available on this platform yet.</BodyText>
    </Card>
  );
}
