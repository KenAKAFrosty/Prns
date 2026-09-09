import * as Bindings from "@prns-internal/expo";
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
  | { readonly tag: "Loading" }
  | { readonly tag: "InspectionFailed" }
  | PrimaryIdentityState;

export function RecoveryScreen() {
  const router = useRouter();
  const { resetDevelopmentData: resetScaffoldData } = useScaffoldState();
  const [inspection, setInspection] = useState<RecoveryState>({ tag: "Loading" });
  const [inspectionAttempt, setInspectionAttempt] = useState(0);
  const [resetting, setResetting] = useState(false);
  const [resetFailure, setResetFailure] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    if (inspectionAttempt > 0) {
      setResetFailure(null);
    }
    if (!("runtime" in runtimeProvider)) {
      setInspection({ tag: "InspectionFailed" });
      return;
    }
    const runtime = runtimeProvider.runtime;
    setInspection({ tag: "Loading" });
    void runtime
      .inspectDevelopmentIdentity()
      .then((identity) => {
        if (active) {
          setInspection(identity);
        }
      })
      .catch(() => {
        if (active) {
          setInspection({ tag: "InspectionFailed" });
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
      if (outcome.tag === Bindings.DevelopmentNodeStopOutcome_Tags.Failed) {
        setResetFailure("App data could not be reset. Nothing else was changed. Try again.");
        return;
      }
      await resetScaffoldData();
      router.replace("/onboarding/welcome");
    } catch {
      setResetFailure("App data could not be reset. Nothing else was changed. Try again.");
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
        {inspection.tag === Bindings.PrimaryIdentityState_Tags.DevelopmentResetRequired ? (
          <Button disabled={resetting} onPress={confirmReset} tone="destructive">
            {resetting ? "Resetting…" : "Reset app data"}
          </Button>
        ) : null}
        {inspection.tag === Bindings.PrimaryIdentityState_Tags.Unavailable ||
        inspection.tag === "InspectionFailed" ? (
          <Button onPress={() => setInspectionAttempt((current) => current + 1)} tone="secondary">
            Retry inspection
          </Button>
        ) : null}
        {inspection.tag === Bindings.PrimaryIdentityState_Tags.Present ? (
          <Button onPress={() => router.replace("/nodes/local")}>Open this device</Button>
        ) : null}
        {inspection.tag === Bindings.PrimaryIdentityState_Tags.Missing ? (
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
  switch (inspection.tag) {
    case "Loading":
      return (
        <Card>
          <Badge>Checking identity</Badge>
          <BodyText>Checking the primary identity…</BodyText>
        </Card>
      );
    case Bindings.PrimaryIdentityState_Tags.Missing:
      return (
        <Card>
          <Subheading>No primary identity</Subheading>
          <BodyText>Create or import an identity through onboarding.</BodyText>
        </Card>
      );
    case Bindings.PrimaryIdentityState_Tags.Present:
      return (
        <Card>
          <Subheading>Primary identity present</Subheading>
          <KeyValue label="Identity hash" value={formatBytes(inspection.inner.identityHash)} />
        </Card>
      );
    case Bindings.PrimaryIdentityState_Tags.Unavailable:
      return (
        <Card>
          <Badge tone="warning">Identity storage unavailable</Badge>
          <BodyText>
            Identity storage is temporarily unavailable. Your data has not been changed. Try again.
          </BodyText>
        </Card>
      );
    case Bindings.PrimaryIdentityState_Tags.DevelopmentResetRequired:
      return (
        <Card>
          <Badge tone="warning">App reset required</Badge>
          <BodyText>
            Stored app data cannot be opened safely. You can reset this preview and start again.
          </BodyText>
        </Card>
      );
    case "InspectionFailed":
      return (
        <Card>
          <Badge tone="warning">Could not check identity</Badge>
          <BodyText>Your identity could not be checked. Your data has not been changed.</BodyText>
        </Card>
      );
  }
}
