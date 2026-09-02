import { useRouter } from "expo-router";

import { screenById } from "@/navigation/catalog";
import { useScaffoldState } from "@/state/scaffold-state-context";
import { identityFixture } from "@/testkit/fixtures";
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
  | "identity-choice"
  | "identity-review"
  | "retention"
  | "provision"
  | "interfaces"
  | "pairing"
  | "complete";

const implementedSteps: readonly OnboardingStep[] = [
  "welcome",
  "identity-choice",
  "identity-review",
];

export function isOnboardingStep(value: string): value is OnboardingStep {
  return [
    "welcome",
    "identity-choice",
    "identity-review",
    "retention",
    "provision",
    "interfaces",
    "pairing",
    "complete",
  ].includes(value);
}

export function OnboardingScreen({ step }: { readonly step: OnboardingStep }) {
  const router = useRouter();
  const { completeOnboardingPreview, onboardingMode, setOnboardingMode, skipOnboardingPreview } =
    useScaffoldState();

  if (!implementedSteps.includes(step)) {
    return <NotYetImplementedScreen entry={screenById("installation.onboarding")} />;
  }

  if (step === "welcome") {
    return (
      <Screen>
        <Badge>Development preview</Badge>
        <ScreenHeading>Welcome to prns</ScreenHeading>
        <BodyText>
          Preview the application shell with synthetic, disposable fixtures. This does not create an
          Installation, identity, or network node.
        </BodyText>
        <CardStack>
          <Button
            onPress={() => {
              setOnboardingMode("import");
              router.push("/onboarding/identity-choice");
            }}
          >
            Import an existing identity
          </Button>
          <Button
            tone="secondary"
            onPress={() => {
              setOnboardingMode("create");
              router.push("/onboarding/identity-choice");
            }}
          >
            Create a new identity
          </Button>
          <Button
            tone="secondary"
            onPress={() => {
              void skipOnboardingPreview().then(() => router.replace("/inbox"));
            }}
          >
            Skip for development
          </Button>
        </CardStack>
      </Screen>
    );
  }

  if (onboardingMode === null) {
    return <OnboardingContextMissing />;
  }

  const fixtureId =
    onboardingMode === "import" ? "identity.imported-preview" : "identity.generated-preview";
  const fixture = identityFixture(fixtureId);
  if (fixture === undefined) {
    return <OnboardingContextMissing />;
  }

  if (step === "identity-choice") {
    return (
      <Screen>
        <Badge>Development preview</Badge>
        <ScreenHeading>
          {onboardingMode === "import" ? "Import identity preview" : "Create identity preview"}
        </ScreenHeading>
        <BodyText>
          The real Rust identity provider is not present. This visibly synthetic fixture lets you
          inspect the intended flow without handling private material.
        </BodyText>
        <Card>
          <Subheading>{fixture.label}</Subheading>
          <KeyValue label="Fingerprint" value={fixture.fingerprint} />
        </Card>
        <CardStack>
          <Button onPress={() => router.push("/onboarding/identity-review")}>Review preview</Button>
          <Button tone="secondary" onPress={() => router.back()}>
            Back
          </Button>
        </CardStack>
      </Screen>
    );
  }

  return (
    <Screen>
      <Badge>Development preview</Badge>
      <ScreenHeading>Review disposable identity preview</ScreenHeading>
      <Card>
        <Subheading>{fixture.label}</Subheading>
        <KeyValue label="Fingerprint" value={fixture.fingerprint} />
      </Card>
      <BodyText>
        Continuing stores only this fixture key and onboarding status. It creates no Reticulum
        credential and carries no backup, recovery, or continuity promise.
      </BodyText>
      <CardStack>
        <Button
          onPress={() => {
            void completeOnboardingPreview(fixture.id).then(() => router.replace("/inbox"));
          }}
        >
          Enter preview
        </Button>
        <Button tone="secondary" onPress={() => router.back()}>
          Back
        </Button>
      </CardStack>
    </Screen>
  );
}

function OnboardingContextMissing() {
  const router = useRouter();
  return (
    <Screen>
      <Badge tone="warning">Preview selection needed</Badge>
      <ScreenHeading>Restart this preview step</ScreenHeading>
      <BodyText>
        The temporary create/import choice is no longer available. No onboarding state was changed.
      </BodyText>
      <Button onPress={() => router.replace("/onboarding/welcome")}>Return to welcome</Button>
    </Screen>
  );
}
