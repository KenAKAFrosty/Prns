import { Disclosure } from "../ui/Disclosure";
import Constants from "expo-constants";
import { Platform, Share } from "react-native";
import { useEffect, useRef, useState } from "react";
import { prns_chat_app as app, prns_service_runtime as native } from "@prns/app-bindings";
import * as platform from "@prns/app-platform";
import { collectSupportFacts, supportReport } from "../runtime/support-report";
import type { SupportFacts } from "../runtime/support-report";
import { useInstallation } from "../runtime/InstallationProvider";
import { useRouteActive } from "../navigation";
import { Button, PageHeader, Panel, Screen, Text } from "../ui/primitives";

export function AboutScreen() {
  const installation = useInstallation();
  const active = useRouteActive();
  const [error, setError] = useState<string>();
  const [sharing, setSharing] = useState(false);
  const generation = useRef(0);
  const configured: unknown = Constants.expoConfig?.extra?.prnsBuild;
  const build = typeof configured === "object" && configured !== null ? configured as { label?: unknown; variant?: unknown } : undefined;
  const label = typeof build?.label === "string" ? build.label : "Local build without a source label";
  const variant = typeof build?.variant === "string" ? build.variant : "Unknown variant";
  const phase = installation.status === undefined ? "Not opened" : native.ServiceRuntimePhase[installation.status.phase];
  const network = installation.network === undefined ? "Not started" : native.ServiceRuntimeNetworkPhase[installation.network.phase];
  const owner = installation.owner;
  const networkKey = installation.networkKey;
  const networkReady = installation.network?.phase === native.ServiceRuntimeNetworkPhase.Ready;
  const [capture, setCapture] = useState<{ owner: typeof owner; networkKey: string | undefined; networkReady: boolean; facts: SupportFacts }>();
  const current = capture?.owner === owner && capture?.networkKey === networkKey && capture?.networkReady === networkReady;
  const loading = !current;
  const facts = current ? capture!.facts : {};
  useEffect(() => {
    if (!active) return;
    setSharing(false);
    const ticket = ++generation.current;
    const abort = new AbortController();
    let reading = false;
    const read = async () => {
      if (reading) return;
      reading = true;
      try {
        const facts = await collectSupportFacts(platform.status, owner, networkReady, abort.signal);
        if (!abort.signal.aborted) setCapture({ owner, networkKey, networkReady, facts });
      } finally { reading = false; }
    };
    void read();
    const timer = setInterval(() => { void read(); }, 5000);
    return () => { abort.abort(); clearInterval(timer); if (generation.current === ticket) generation.current++; };
  }, [active, owner, networkKey, networkReady]);
  const report = supportReport({ version: Constants.nativeAppVersion ?? Constants.expoConfig?.version ?? "unknown",
    nativeBuild: Constants.nativeBuildVersion ?? "unavailable", label, variant,
    platform: `${Platform.OS} ${Platform.Version}`, api: app.appApiVersion(), local: phase, network,
    localFailure: installation.status?.failure !== undefined,
    networkFailure: installation.network?.failure !== undefined || installation.error !== undefined }, facts);
  return <Screen><PageHeader title="About PRNS Chat">
    <Button title={sharing ? "Opening share…" : "Share diagnostics"} icon="share" size="compact" disabled={sharing || loading} onPress={() => {
      const ticket = generation.current;
      setSharing(true); setError(undefined);
      void Share.share({ title: "PRNS Chat diagnostics", message: report })
        .catch((cause: unknown) => { if (generation.current === ticket) setError(String(cause)); })
        .finally(() => { if (generation.current === ticket) setSharing(false); });
    }} />
    </PageHeader>
    <Panel><Text>Version {Constants.nativeAppVersion ?? Constants.expoConfig?.version ?? "unknown"} ({Constants.nativeBuildVersion ?? "unavailable"})</Text>
      <Text selectable>{label}</Text><Text>{variant}</Text></Panel>
    <Text>Conversations, contacts and connected nodes.</Text>
    <Disclosure label="Diagnostics">
      {loading && <Text>Checking connection health…</Text>}
      <Text selectable>{report}</Text>
    </Disclosure>
    {error !== undefined && <Text accessibilityRole="alert">{error}</Text>}
  </Screen>;
}
