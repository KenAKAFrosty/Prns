import type { DevelopmentNodeSnapshot } from "@prns-internal/expo";
import { type PropsWithChildren, useEffect, useRef, useState } from "react";
import { Pressable, StyleSheet, Text, useWindowDimensions, View } from "react-native";

import { bluetoothStatus } from "@/features/connections/bluetooth-status";
import { useMessagingDirectory } from "@/features/contacts/messaging-directory";
import { peerLabel } from "@/features/inbox/format";
import { formatBytes, formatRuntime } from "@/features/nodes/format";
import {
  type DevelopmentRuntimeView,
  useDevelopmentRuntime,
} from "@/native/development-runtime-context";
import { NavigationLink } from "@/ui/navigation-link";
import {
  ActionRow,
  Badge,
  BodyText,
  Button,
  Card,
  CardHeader,
  KeyValue,
  Screen,
  ScreenHeading,
} from "@/ui/primitives";
import { radius, space, useAppPalette } from "@/ui/theme";
import {
  announceIngressName,
  formatNetworkDuration,
  formatRouteExpiry,
  formatTraffic,
  hopLabel,
  logicalInterfaceName,
  shortNetworkId,
} from "./format";

export type NetworkTab = "connections" | "routes" | "announcements";
const pageSize = 20;

export function NetworkScreen({
  initialTab = "connections",
}: {
  readonly initialTab?: NetworkTab;
}) {
  const runtime = useDevelopmentRuntime();
  return (
    <NetworkSession
      key={runtime.snapshot?.generationId.toString() ?? "unavailable"}
      runtime={runtime}
      initialTab={initialTab}
    />
  );
}

function NetworkSession({
  runtime,
  initialTab,
}: {
  readonly runtime: DevelopmentRuntimeView;
  readonly initialTab: NetworkTab;
}) {
  const directory = useMessagingDirectory();
  const destinationLabel = (destination: Uint8Array) =>
    peerLabel(destination, directory.peers, directory.contacts);
  const [tab, setTab] = useState(initialTab);
  const [visibleAnnouncements, setVisibleAnnouncements] = useState(pageSize);
  const [pending, setPending] = useState<"refresh" | "clear" | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [clearedRevision, setClearedRevision] = useState<bigint | null>(null);
  const pendingRef = useRef(false);
  const mounted = useRef(true);
  const latestRuntime = useRef(runtime);
  latestRuntime.current = runtime;
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => setTab(initialTab), [initialTab]);

  const snapshot = runtime.snapshot;
  const network = snapshot?.network;
  const status = bluetoothStatus(runtime);
  const peers = status.showConnectedPeers
    ? (snapshot?.bluetooth.peers.filter((peer) => peer.connected) ?? [])
    : [];
  const routes = network?.state.tag === "Ready" ? network.routes : [];
  const providerClearRevision =
    runtime.networkActivityClear?.generationId === snapshot?.generationId
      ? (runtime.networkActivityClear?.activityRevision ?? null)
      : null;
  const clearBarrier =
    providerClearRevision === null
      ? clearedRevision
      : clearedRevision !== null && clearedRevision > providerClearRevision
        ? clearedRevision
        : providerClearRevision;
  // A clear acknowledgement may arrive even if the subsequent refresh fails.
  // Hide only older projections, never announcements admitted after that clear.
  const awaitingClearRefresh =
    clearBarrier !== null &&
    network !== undefined &&
    network.state.tag !== "Stopped" &&
    network.state.tag !== "Starting" &&
    network.activityRevision < clearBarrier;
  const announcements = awaitingClearRefresh ? [] : (network?.announces ?? []);
  const canClear =
    snapshot !== null &&
    network !== undefined &&
    network.state.tag !== "Stopped" &&
    network.state.tag !== "Starting" &&
    !awaitingClearRefresh &&
    (announcements.length > 0 || network.droppedAnnounceCount > 0n);

  const refresh = async () => {
    if (pendingRef.current) return;
    pendingRef.current = true;
    setPending("refresh");
    setFeedback(null);
    try {
      await runtime.androidRuntime?.refresh();
      const result = await runtime.refreshSnapshot();
      await directory.refresh();
      if (mounted.current && result.type === "operationFailure")
        setFeedback("The network view could not be refreshed. Try again.");
    } catch {
      if (mounted.current) setFeedback("The network view could not be refreshed. Try again.");
    } finally {
      pendingRef.current = false;
      if (mounted.current) setPending(null);
    }
  };

  const clear = async () => {
    if (pendingRef.current || !canClear || snapshot === null) return;
    const generationId = snapshot.generationId;
    pendingRef.current = true;
    setPending("clear");
    setFeedback(null);
    const stillCurrent = () =>
      mounted.current && latestRuntime.current.snapshot?.generationId === generationId;
    try {
      const result = await runtime.clearNetworkActivity({ generationId });
      if (!stillCurrent()) return;
      if (result.type === "operationFailure") {
        setFeedback("Announcement history could not be cleared. Try again.");
      } else {
        switch (result.outcome.tag) {
          case "Cleared": {
            const revision = result.outcome.inner.activityRevision;
            setClearedRevision((previous) =>
              previous !== null && previous > revision ? previous : revision,
            );
            setVisibleAnnouncements(pageSize);
            setFeedback("Earlier announcement history cleared.");
            break;
          }
          case "GenerationChanged":
            setFeedback("The node restarted. Refresh before clearing its new history.");
            break;
          case "LocalNodeStopped":
            setFeedback("The node has stopped. Its announcement history is no longer active.");
            break;
          case "Busy":
            setFeedback("The node is busy. Wait a moment, then try again.");
            break;
        }
      }
    } catch {
      if (stillCurrent()) setFeedback("Announcement history could not be cleared. Try again.");
    } finally {
      pendingRef.current = false;
      if (stillCurrent()) setPending(null);
    }
  };

  return (
    <Screen>
      <ScreenHeading>Network</ScreenHeading>
      <Card>
        <CardHeader title="This phone">
          <Badge>{snapshot === null ? "Getting ready" : formatRuntime(snapshot.runtime)}</Badge>
        </CardHeader>
        <BodyText muted>Times reflect the last network update.</BodyText>
        <ActionRow>
          <Button
            disabled={pending !== null || runtime.availability.type !== "available"}
            onPress={() => void refresh()}
            tone="secondary"
          >
            {pending === "refresh" ? "Refreshing…" : "Refresh network"}
          </Button>
          {runtime.canStartNode ? <Button onPress={runtime.startNode}>Start node</Button> : null}
        </ActionRow>
        {feedback === null ? null : <BodyText>{feedback}</BodyText>}
      </Card>

      <NetworkTabs
        selected={tab}
        onSelect={setTab}
        counts={{
          connections: peers.length,
          routes: routes.length,
          announcements: announcements.length,
        }}
      />

      {tab === "connections" ? (
        <Card>
          <CardHeader title="Bluetooth connections">
            <Badge tone={status.warning ? "warning" : "neutral"}>{status.label}</Badge>
          </CardHeader>
          <BodyText muted>{status.description}</BodyText>
          {peers.length === 0 ? null : (
            <BodyText muted>
              Traffic totals below are for each connection, not the whole node.
            </BodyText>
          )}
          {peers.map((peer) => (
            <NetworkRow
              key={formatBytes(peer.interfaceId)}
              title={peer.name ?? "Nearby device"}
              summary={`This connection: received ${formatTraffic(peer.rxBytes)} · sent ${formatTraffic(peer.txBytes)}`}
              detailLabel={`Connection details for ${peer.name ?? shortNetworkId(peer.interfaceId)}`}
            >
              {peer.rssiDbm === undefined ? null : (
                <KeyValue label="Signal" value={`${peer.rssiDbm} dBm`} />
              )}
              {peer.details === undefined ? null : <BodyText muted>{peer.details}</BodyText>}
              <KeyValue label="Received on this connection" value={`${peer.rxBytes} bytes`} />
              <KeyValue label="Sent on this connection" value={`${peer.txBytes} bytes`} />
              <KeyValue label="Connection ID" value={formatBytes(peer.interfaceId)} />
            </NetworkRow>
          ))}
          <NavigationLink href="/more/interfaces">Connection settings</NavigationLink>
        </Card>
      ) : null}

      {tab === "routes" ? (
        <Card>
          <CardHeader title="Known routes">
            <Badge>{routes.length}</Badge>
          </CardHeader>
          <BodyText muted>
            Known paths to destinations. A route is not a connected device or proof of a
            message&apos;s path.
          </BodyText>
          <NetworkInspectionState
            snapshot={snapshot}
            unavailable={runtime.availability.type === "unavailable"}
          />
          {network?.state.tag === "Ready" && routes.length === 0 ? (
            <BodyText>
              No routes recorded yet. Nearby devices can announce themselves to make destinations
              known.
            </BodyText>
          ) : null}
          {snapshot === null
            ? null
            : routes.map((route) => (
                <NetworkRow
                  key={`${formatBytes(route.destination)}:${formatBytes(route.interfaceId)}`}
                  title={destinationLabel(route.destination)}
                  summary={`${hopLabel(route.hops)} · ${logicalInterfaceName(snapshot, route.interfaceId)} · ${formatRouteExpiry(route)}`}
                  detailLabel={`Route details for ${destinationLabel(route.destination)}`}
                >
                  <KeyValue
                    label="Learned"
                    value={`${formatNetworkDuration(route.learnedAgeMillis)} ago`}
                  />
                  <KeyValue
                    label="Last activity"
                    value={`${formatNetworkDuration(route.lastActivityAgeMillis)} ago`}
                  />
                  <KeyValue label="Destination" value={formatBytes(route.destination)} />
                  <KeyValue label="Logical interface ID" value={formatBytes(route.interfaceId)} />
                  <KeyValue
                    label="Relay identity"
                    value={
                      route.viaIdentity === undefined
                        ? "Not recorded"
                        : formatBytes(route.viaIdentity)
                    }
                  />
                </NetworkRow>
              ))}
        </Card>
      ) : null}

      {tab === "announcements" ? (
        <Card>
          <CardHeader title="Heard announcements">
            <Badge>{announcements.length}</Badge>
          </CardHeader>
          <BodyText muted>
            The latest 200 verified announcements since the node started. A past announcement does
            not mean a device is still connected.
          </BodyText>
          <BodyText muted>Names come from your contacts and latest discovery.</BodyText>
          {network?.state.tag === "Unavailable" && announcements.length > 0 ? (
            <BodyText>
              Route inspection is unavailable. Earlier announcements are still shown.
            </BodyText>
          ) : null}
          {awaitingClearRefresh ? (
            <BodyText>History cleared. Refresh to load announcements received since then.</BodyText>
          ) : announcements.length === 0 ? (
            network?.state.tag === "Ready" || network?.state.tag === "Unavailable" ? (
              <BodyText>No announcements heard yet. Ask a nearby node to announce.</BodyText>
            ) : (
              <NetworkInspectionState
                snapshot={snapshot}
                unavailable={runtime.availability.type === "unavailable"}
              />
            )
          ) : null}
          {!awaitingClearRefresh && network !== undefined && network.droppedAnnounceCount > 0n ? (
            <BodyText muted>
              {network.droppedAnnounceCount.toString()} older entries removed. This list keeps the
              latest 200.
            </BodyText>
          ) : null}
          {snapshot === null
            ? null
            : announcements.slice(0, visibleAnnouncements).map((announce) => (
                <NetworkRow
                  key={`${snapshot.generationId}:${announce.recordId}`}
                  title={destinationLabel(announce.destination)}
                  summary={`${formatNetworkDuration(announce.ageMillis)} ago · ${hopLabel(announce.hops)} · ${announceIngressName(snapshot, announce.sourceInterface)}`}
                  detailLabel={`Announcement details for ${destinationLabel(announce.destination)}, entry ${announce.recordId}`}
                >
                  <KeyValue
                    label="Type"
                    value={announce.isPathResponse ? "Path response" : "Announcement"}
                  />
                  <KeyValue label="Destination" value={formatBytes(announce.destination)} />
                  <KeyValue
                    label="Announced identity"
                    value={formatBytes(announce.announcedIdentity)}
                  />
                  <KeyValue
                    label="Received through"
                    value={announceIngressName(snapshot, announce.sourceInterface)}
                  />
                  <KeyValue
                    label="Recorded connection ID"
                    value={formatBytes(announce.sourceInterface)}
                  />
                </NetworkRow>
              ))}
          {announcements.length > visibleAnnouncements ? (
            <Button
              onPress={() => setVisibleAnnouncements((count) => count + pageSize)}
              tone="secondary"
            >
              Show more ({announcements.length - visibleAnnouncements} remaining)
            </Button>
          ) : null}
          <Button
            disabled={pending !== null || !canClear}
            onPress={() => void clear()}
            tone="secondary"
          >
            {pending === "clear" ? "Clearing…" : "Clear announcement history"}
          </Button>
          <BodyText muted>
            Clearing this list does not remove contacts, routes, or messages.
          </BodyText>
        </Card>
      ) : null}
      <NavigationLink href="/more" direction="back">
        Back to More
      </NavigationLink>
    </Screen>
  );
}

function NetworkInspectionState({
  snapshot,
  unavailable,
}: {
  readonly snapshot: DevelopmentNodeSnapshot | null;
  readonly unavailable: boolean;
}) {
  if (unavailable) return <BodyText>Network inspection needs the iOS or Android app.</BodyText>;
  switch (snapshot?.network.state.tag) {
    case "Ready":
      return null;
    case "Stopped":
      return <BodyText>Start this phone&apos;s node to inspect its network.</BodyText>;
    case "Unavailable":
      return (
        <BodyText>
          Network inspection is unavailable. Connections may still be working. Try refreshing.
        </BodyText>
      );
    default:
      return <BodyText>Preparing the network view…</BodyText>;
  }
}

function NetworkTabs({
  selected,
  onSelect,
  counts,
}: {
  readonly selected: NetworkTab;
  readonly onSelect: (tab: NetworkTab) => void;
  readonly counts: Readonly<Record<NetworkTab, number>>;
}) {
  const palette = useAppPalette();
  const { fontScale } = useWindowDimensions();
  return (
    <View accessibilityRole="tablist" style={styles.tabs}>
      {(["connections", "routes", "announcements"] as const).map((tab) => (
        <Pressable
          accessibilityRole="tab"
          accessibilityState={{ selected: selected === tab }}
          key={tab}
          onPress={() => onSelect(tab)}
          style={[
            styles.tab,
            fontScale >= 1.4 ? styles.stackedTab : null,
            {
              backgroundColor: selected === tab ? palette.selected : palette.surface,
              borderColor: palette.border,
            },
          ]}
        >
          <Text
            style={[
              styles.tabText,
              { color: selected === tab ? palette.selectedText : palette.text },
            ]}
          >
            {tab === "connections" ? "Connections" : tab === "routes" ? "Routes" : "Announcements"}{" "}
            {counts[tab]}
          </Text>
        </Pressable>
      ))}
    </View>
  );
}

function NetworkRow({
  title,
  summary,
  detailLabel,
  children,
}: PropsWithChildren<{
  readonly title: string;
  readonly summary: string;
  readonly detailLabel: string;
}>) {
  const [expanded, setExpanded] = useState(false);
  const palette = useAppPalette();
  return (
    <View style={[styles.row, { borderColor: palette.border }]}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={detailLabel}
        accessibilityState={{ expanded }}
        onPress={() => setExpanded((value) => !value)}
        style={styles.rowButton}
      >
        <View style={styles.rowTitle}>
          <Text style={[styles.title, { color: palette.text }]}>{title}</Text>
          <Text
            accessibilityElementsHidden
            importantForAccessibility="no"
            style={{ color: palette.textMuted }}
          >
            {expanded ? "−" : "+"}
          </Text>
        </View>
        <BodyText muted>{summary}</BodyText>
      </Pressable>
      {expanded ? <View style={styles.details}>{children}</View> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  tabs: { flexDirection: "row", flexWrap: "wrap", gap: space.sm },
  tab: {
    flexGrow: 1,
    flexBasis: 100,
    minHeight: 48,
    justifyContent: "center",
    padding: space.sm,
    borderWidth: 1,
    borderRadius: radius.sm,
  },
  stackedTab: { flexBasis: "100%" },
  tabText: { fontSize: 14, fontWeight: "600", textAlign: "center" },
  row: { borderTopWidth: StyleSheet.hairlineWidth },
  rowButton: { minHeight: 48, paddingVertical: space.sm, gap: space.xs },
  rowTitle: { flexDirection: "row", gap: space.sm, alignItems: "flex-start" },
  title: { flex: 1, flexShrink: 1, fontSize: 16, fontWeight: "600" },
  details: { gap: space.sm, paddingBottom: space.sm },
});
