import type { PrimaryIdentityState } from "@prns-internal/expo";
import { useRouter } from "expo-router";
import { useEffect, useState } from "react";
import { Alert } from "react-native";

import { formatBytes } from "@/features/nodes/format";
import { runtimeProvider } from "@/native/runtime-provider";
import { useScaffoldState } from "@/state/scaffold-state-context";
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

type RecoveryState =
  | { readonly type: "loading" }
  | { readonly type: "inspectionFailed"; readonly detail: string }
  | PrimaryIdentityState;

export function RecoveryScreen() {
  const router = useRouter();
  const { resetDevelopmentData: resetScaffoldData } = useScaffoldState();
  const [inspection, setInspection] = useState<RecoveryState>({ type: "loading" });
  const [inspectionAttempt, setInspectionAttempt] = useState(0);
  const [resetting, setResetting] = useState(false);
  const [resetFailure, setResetFailure] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    if (inspectionAttempt > 0) {
      setResetFailure(null);
    }
    if (!("runtime" in runtimeProvider)) {
      setInspection({
        type: "inspectionFailed",
        detail: `Identity recovery is not available on ${runtimeProvider.availability.platform} yet.`,
      });
      return;
    }
    const runtime = runtimeProvider.runtime;
    setInspection({ type: "loading" });
    void runtime
      .inspectDevelopmentIdentity()
      .then((identity) => {
        if (active) {
          setInspection(identity);
        }
      })
      .catch((error: unknown) => {
        if (active) {
          setInspection({
            type: "inspectionFailed",
            detail: error instanceof Error ? error.message : String(error),
          });
        }
      });
    return () => {
      active = false;
    };
  }, [inspectionAttempt]);

  const reset = async () => {
    if (!("runtime" in runtimeProvider)) {
      return;
    }
    setResetting(true);
    setResetFailure(null);
    try {
      const outcome = await runtimeProvider.runtime.resetDevelopmentData();
      if (outcome.type === "failed") {
        setResetFailure(`${outcome.stage}: ${outcome.detail}`);
        return;
      }
      await resetScaffoldData();
      router.replace("/onboarding/welcome");
    } catch (error) {
      setResetFailure(error instanceof Error ? error.message : String(error));
    } finally {
      setResetting(false);
    }
  };

  const confirmReset = () => {
    Alert.alert(
      "Reset app data?",
      "This permanently removes your primary identity, paired-node access, saved contacts, messages, and app preferences from this device.",
      [
        { text: "Cancel", style: "cancel" },
        { text: "Reset", style: "destructive", onPress: () => void reset() },
      ],
    );
  };

  return (
    <Screen>
      <Badge>Identity</Badge>
      <ScreenHeading>Recovery</ScreenHeading>
      <RecoveryStateCard inspection={inspection} />
      {resetFailure === null ? null : (
        <Card>
          <Badge tone="warning">Reset failed</Badge>
          <BodyText>{resetFailure}</BodyText>
        </Card>
      )}
      <CardStack>
        {inspection.type === "developmentResetRequired" ? (
          <Button disabled={resetting} onPress={confirmReset} tone="destructive">
            {resetting ? "Resetting…" : "Reset app data"}
          </Button>
        ) : null}
        {inspection.type === "unavailable" || inspection.type === "inspectionFailed" ? (
          <Button onPress={() => setInspectionAttempt((current) => current + 1)} tone="secondary">
            Retry inspection
          </Button>
        ) : null}
        {inspection.type === "present" ? (
          <Button onPress={() => router.replace("/nodes/local")}>Open local node</Button>
        ) : null}
        {inspection.type === "missing" ? (
          <Button onPress={() => router.replace("/onboarding/welcome")}>
            Return to onboarding
          </Button>
        ) : null}
      </CardStack>
      <BodyText muted>
        Reset is permanent. Identity repair, export, and backup are not available in this preview.
      </BodyText>
    </Screen>
  );
}

function RecoveryStateCard({ inspection }: { readonly inspection: RecoveryState }) {
  switch (inspection.type) {
    case "loading":
      return (
        <Card>
          <Badge>Checking identity</Badge>
          <BodyText>Checking the primary identity…</BodyText>
        </Card>
      );
    case "missing":
      return (
        <Card>
          <Subheading>No primary identity</Subheading>
          <BodyText>Create or import an identity through onboarding.</BodyText>
        </Card>
      );
    case "present":
      return (
        <Card>
          <Subheading>Primary identity present</Subheading>
          <KeyValue label="Identity hash" value={formatBytes(inspection.identityHash)} />
        </Card>
      );
    case "unavailable":
      return (
        <Card>
          <Badge tone="warning">Identity storage unavailable</Badge>
          <BodyText>{inspection.detail}</BodyText>
          <BodyText muted>
            Operational storage failures do not suggest deleting identity data.
          </BodyText>
        </Card>
      );
    case "developmentResetRequired":
      return (
        <Card>
          <Badge tone="warning">App reset required</Badge>
          <BodyText>{inspection.reason}</BodyText>
        </Card>
      );
    case "inspectionFailed":
      return (
        <Card>
          <Badge tone="warning">Could not check identity</Badge>
          <BodyText>{inspection.detail}</BodyText>
        </Card>
      );
  }
}
