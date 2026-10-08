import * as Bindings from "@prns-internal/expo";
import { useFocusEffect } from "expo-router";
import { useCallback, useEffect, useRef, useState } from "react";

import type { DevelopmentRuntimeView } from "@/native/development-runtime-context";
import { BodyText, Button, Card, CardHeader } from "@/ui/primitives";
import { formatBytes } from "./format";
import {
  RemoteChangeStatusCard,
  remoteManagementFailureMessage,
} from "./remote-management/change-status";
import { RemoteInterfaceCard } from "./remote-management/interface-card";
import { NodeControls } from "./remote-management/node-controls";
import { NodeOverviewCard } from "./remote-management/node-overview";
import { ManagementTabs } from "./remote-management/management-tabs";
import { interfaceKindLabel } from "./remote-management/interface-format";
import { ControllerAccessCard } from "./remote-management/controller-access";
import { WifiSetupCard, supportsWifiSetup } from "./remote-management/wifi-setup";

const maximumEntries = 128;

/** A focused view owns reads and drafts; the native actor owns accepted writes. */
export function RemoteManagementPanel({
  target,
  runtime,
}: {
  readonly target: Uint8Array;
  readonly runtime: DevelopmentRuntimeView;
}) {
  const [overview, setOverview] = useState<Bindings.RemoteNodeOverview>();
  const [interfaces, setInterfaces] = useState<Bindings.RemoteInterfacePage>();
  const [details, setDetails] = useState<Record<string, Bindings.RemoteInterfaceDetails>>({});
  const [peers, setPeers] = useState<Record<string, Bindings.RemotePeerPage>>({});
  const [controllers, setControllers] = useState<Bindings.RemoteControllerPage>();
  const [available, setAvailable] = useState<Bindings.RemoteControlRequestKind[]>([]);
  const [viewRevision, setViewRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const [fresh, setFresh] = useState(false);
  const [focusRevision, setFocusRevision] = useState(0);
  const [section, setSection] = useState("interfaces");
  const [selectedInterface, setSelectedInterface] = useState<string>();
  const [failure, setFailure] = useState<string>();
  const [accepted, setAccepted] = useState<Bindings.RemoteChangeOperation>();
  const [uncertain, setUncertain] = useState<{ previousId: bigint | undefined }>();
  const request = useRef<AbortController | null>(null);
  const focused = useRef(false);
  const writeInFlight = useRef(false);
  const autoReadPending = useRef(false);
  const targetId = formatBytes(target);

  useFocusEffect(
    // biome-ignore lint/correctness/useExhaustiveDependencies: target and native lifetime changes invalidate a focused read.
    useCallback(() => {
      focused.current = true;
      setBusy(false);
      // A fresh live read is required after returning to this screen.
      setAvailable([]);
      setFresh(false);
      setFailure(undefined);
      autoReadPending.current = true;
      setFocusRevision((revision) => revision + 1);
      return () => {
        focused.current = false;
        autoReadPending.current = false;
        request.current?.abort();
        request.current = null;
      };
    }, [targetId, runtime.snapshot?.generationId]),
  );

  const stored = runtime.snapshot?.lastRemoteChange;
  const targetOperation =
    stored !== undefined && formatBytes(stored.targetIdentityFingerprint) === formatBytes(target)
      ? stored
      : undefined;
  useEffect(() => {
    if (targetOperation !== undefined) {
      setAccepted((previous) =>
        previous === undefined || targetOperation.operationId >= previous.operationId
          ? targetOperation
          : previous,
      );
    }
  }, [targetOperation]);
  const latest =
    targetOperation !== undefined &&
    (accepted === undefined || targetOperation.operationId >= accepted.operationId)
      ? targetOperation
      : accepted;
  const operation = reconcileRemoteChange(latest, stored);
  const admissionUncertain =
    uncertain !== undefined &&
    (operation === undefined || operation.operationId === uncertain.previousId);
  const pending =
    operation?.generationId === runtime.snapshot?.generationId &&
    operation?.status.tag === Bindings.RemoteChangeStatus_Tags.Pending;
  const blocked =
    busy || pending || admissionUncertain || runtime.snapshot?.activeOperation !== undefined;

  const read = useCallback(
    async (query: Bindings.RemoteNodeQuery) => {
      if (
        !focused.current ||
        runtime.snapshot?.runtime !== Bindings.DevelopmentNodeRuntime.Running ||
        request.current !== null ||
        writeInFlight.current ||
        pending ||
        admissionUncertain ||
        runtime.snapshot?.activeOperation !== undefined
      )
        return;
      const controller = new AbortController();
      request.current = controller;
      setBusy(true);
      setFailure(undefined);
      const result = await runtime.readRemoteNode(
        { targetIdentityFingerprint: target, query },
        controller.signal,
      );
      if (request.current !== controller || controller.signal.aborted || !focused.current) return;
      request.current = null;
      setBusy(false);
      if (result.type === "operationFailure") {
        setAvailable([]);
        setFresh(false);
        setFailure("The node could not be read. Check its connection and try again.");
        return;
      }
      const outcome = result.outcome;
      if (outcome.tag === Bindings.ReadRemoteNodeOutcome_Tags.Busy) {
        setFailure("Another operation is in progress. Wait for it to finish, then try again.");
        return;
      }
      if (outcome.tag === Bindings.ReadRemoteNodeOutcome_Tags.Failed) {
        setAvailable([]);
        setFresh(false);
        setFailure(remoteManagementFailureMessage(outcome.inner.stage));
        return;
      }
      setAvailable(outcome.inner.availableRequests);
      const data = outcome.inner.data;
      switch (data.tag) {
        case Bindings.RemoteNodeData_Tags.Overview:
          setFresh(true);
          setViewRevision((revision) => revision + 1);
          setOverview(data.inner.overview);
          setInterfaces(data.inner.overview.interfaces);
          setDetails({});
          setPeers({});
          setControllers(undefined);
          break;
        case Bindings.RemoteNodeData_Tags.Interfaces:
          try {
            setInterfaces(appendPage(interfaces, data.inner.page, (entry) => entry.interfaceId));
          } catch {
            setFailure(
              "The interface list changed while loading. Refresh the node to start again.",
            );
          }
          break;
        case Bindings.RemoteNodeData_Tags.Interface:
          setViewRevision((revision) => revision + 1);
          setDetails((previous) => ({
            ...previous,
            [formatBytes(data.inner.details.interfaceId)]: data.inner.details,
          }));
          break;
        case Bindings.RemoteNodeData_Tags.Peers: {
          const page = data.inner.page;
          const id = formatBytes(page.interfaceId);
          const previous =
            query.tag === Bindings.RemoteNodeQuery_Tags.Peers && query.inner.after !== undefined
              ? peers[id]
              : undefined;
          try {
            const merged = { ...page, ...appendPage(previous, page, (entry) => entry.peerId) };
            setPeers((all) => ({ ...all, [id]: merged }));
          } catch {
            setFailure("The peer list changed while loading. Refresh peers to start again.");
          }
          break;
        }
        case Bindings.RemoteNodeData_Tags.Controllers: {
          const page = data.inner.page;
          const previous =
            query.tag === Bindings.RemoteNodeQuery_Tags.Controllers &&
            query.inner.after !== undefined
              ? controllers
              : undefined;
          try {
            const merged = appendPage(
              previous === undefined
                ? undefined
                : { entries: previous.identities, next: previous.next },
              { entries: page.identities, next: page.next },
              (identity) => identity,
            );
            setControllers({ identities: merged.entries, next: merged.next });
          } catch {
            setFailure("The device list changed while loading. Refresh devices to start again.");
          }
          break;
        }
      }
    },
    [runtime, target, pending, admissionUncertain, interfaces, peers, controllers],
  );

  useEffect(() => {
    // Defer the single focus read while the actor is occupied. A failed read
    // needs an explicit retry; a render or snapshot update is not a retry loop.
    if (
      focusRevision === 0 ||
      !focused.current ||
      !autoReadPending.current ||
      blocked ||
      writeInFlight.current ||
      runtime.snapshot?.runtime !== Bindings.DevelopmentNodeRuntime.Running
    )
      return;
    autoReadPending.current = false;
    void read(Bindings.RemoteNodeQuery.Overview.new());
  }, [blocked, focusRevision, read, runtime.snapshot?.runtime]);

  const change = async (next: Bindings.RemoteNodeChange) => {
    if (
      !focused.current ||
      runtime.snapshot?.runtime !== Bindings.DevelopmentNodeRuntime.Running ||
      blocked ||
      writeInFlight.current ||
      request.current !== null
    )
      return;
    writeInFlight.current = true;
    setBusy(true);
    setFailure(undefined);
    const previousId = operation?.operationId;
    const result = await runtime.changeRemoteNode({
      targetIdentityFingerprint: target,
      change: next,
    });
    writeInFlight.current = false;
    if (!focused.current) return;
    setBusy(false);
    if (result.type === "operationFailure") {
      // A lost admission reply is not evidence that a write was rejected.
      setUncertain({ previousId });
      setAvailable([]);
      setFresh(false);
      await runtime.refreshSnapshot();
    } else if (result.outcome.tag === Bindings.ChangeRemoteNodeOutcome_Tags.Accepted) {
      setAccepted(result.outcome.inner.operation);
      setUncertain(undefined);
      // Do not edit again from observations made before this write.
      setAvailable([]);
      setFresh(false);
      await runtime.refreshSnapshot();
    } else {
      setFailure(
        result.outcome.tag === Bindings.ChangeRemoteNodeOutcome_Tags.Busy
          ? "Another operation is in progress. Wait for it to finish, then try again."
          : remoteManagementFailureMessage(result.outcome.inner.stage),
      );
    }
  };

  const selected =
    interfaces?.entries.find((entry) => formatBytes(entry.interfaceId) === selectedInterface) ??
    interfaces?.entries[0];
  const visibleInterfaces = selected === undefined ? [] : [selected];
  const controllerIdentity = runtime.snapshot?.pairedTargets?.find(
    (candidate) => formatBytes(candidate.targetIdentityFingerprint) === targetId,
  )?.controllerIdentityFingerprint;

  return (
    <>
      <Card>
        <CardHeader title="Node information">
          <Button
            disabled={blocked}
            tone="secondary"
            accessibilityLabel={
              busy ? "Loading…" : failure !== undefined ? "Try again" : "Refresh node information"
            }
            onPress={() => void read(Bindings.RemoteNodeQuery.Overview.new())}
          >
            {busy ? "Loading…" : failure !== undefined ? "Try again" : "Refresh"}
          </Button>
        </CardHeader>
        {fresh && !blocked ? null : (
          <BodyText muted>
            {busy
              ? overview === undefined
                ? "Loading node settings…"
                : "Refreshing node information…"
              : pending
                ? "Waiting for the current change to finish…"
                : runtime.snapshot?.activeOperation !== undefined
                  ? "Waiting for another operation to finish…"
                  : overview === undefined
                    ? "Settings will appear when the node responds."
                    : "Showing the last information received. Refresh before changing settings."}
          </BodyText>
        )}
        {failure === undefined ? null : <BodyText>{failure}</BodyText>}
        {admissionUncertain ? (
          <>
            <BodyText>
              The app could not confirm whether the change was accepted. It has not been repeated.
              Check the node before trying again.
            </BodyText>
            <Button tone="secondary" onPress={() => void runtime.refreshSnapshot()}>
              Check change status
            </Button>
          </>
        ) : null}
      </Card>
      {operation === undefined ? null : <RemoteChangeStatusCard operation={operation} />}
      {overview === undefined ? null : (
        <ManagementTabs
          label="Node settings sections"
          options={[
            { value: "interfaces", label: "Interfaces" },
            { value: "device", label: "Device" },
            { value: "information", label: "Information", shortLabel: "Info" },
            ...(available.includes(Bindings.RemoteControlRequestKind.InventoryControllers) ||
            controllers !== undefined
              ? [{ value: "access", label: "Access" }]
              : []),
          ]}
          value={section}
          onChange={setSection}
        />
      )}
      {overview !== undefined && section === "information" ? (
        <NodeOverviewCard overview={overview} />
      ) : null}
      {overview !== undefined && section === "interfaces" ? (
        <>
          {interfaces === undefined ? (
            <BodyText>This node does not share its interfaces.</BodyText>
          ) : interfaces.entries.length === 0 ? (
            <BodyText>No interfaces were reported.</BodyText>
          ) : (
            <ManagementTabs
              label="Node interfaces"
              options={interfaces.entries.map((entry, index) => ({
                value: formatBytes(entry.interfaceId),
                label: `${interfaceKindLabel(entry.kind)}${interfaces.entries.filter((other) => other.kind === entry.kind).length > 1 ? ` ${index + 1}` : ""}`,
              }))}
              value={selected === undefined ? "" : formatBytes(selected.interfaceId)}
              onChange={setSelectedInterface}
            />
          )}
          {visibleInterfaces.map((entry) => {
            const id = formatBytes(entry.interfaceId);
            const peerPage = peers[id];
            return (
              <RemoteInterfaceCard
                key={`${id}:${viewRevision}`}
                entry={entry}
                details={details[id]}
                peers={peerPage}
                availableRequests={available}
                busy={blocked}
                onLoadDetails={() =>
                  void read(
                    Bindings.RemoteNodeQuery.Interface.new({ interfaceId: entry.interfaceId }),
                  )
                }
                onLoadPeers={() =>
                  void read(
                    Bindings.RemoteNodeQuery.Peers.new({
                      interfaceId: entry.interfaceId,
                      after: undefined,
                    }),
                  )
                }
                onLoadMorePeers={
                  peerPage?.next === undefined || peerPage.entries.length >= maximumEntries
                    ? undefined
                    : () =>
                        void read(
                          Bindings.RemoteNodeQuery.Peers.new({
                            interfaceId: entry.interfaceId,
                            after: peerPage.next,
                          }),
                        )
                }
                onChange={(next) => void change(next)}
              />
            );
          })}
          {interfaces?.next === undefined ? null : interfaces.entries.length >= maximumEntries ? (
            <BodyText>
              Showing the first {maximumEntries} interfaces. Refresh to reload this list.
            </BodyText>
          ) : (
            <Button
              tone="secondary"
              disabled={blocked}
              onPress={() =>
                void read(Bindings.RemoteNodeQuery.Interfaces.new({ after: interfaces.next }))
              }
            >
              Load more interfaces
            </Button>
          )}
        </>
      ) : null}
      {overview !== undefined && section === "device" ? (
        <>
          {supportsWifiSetup(available) ? (
            <WifiSetupCard target={target} runtime={runtime} busy={blocked} />
          ) : null}
          <NodeControls
            availableRequests={available}
            busy={blocked}
            onChange={(next) => void change(next)}
          />
        </>
      ) : null}
      {overview !== undefined && section === "access" && controllerIdentity !== undefined ? (
        <>
          {available.includes(Bindings.RemoteControlRequestKind.InventoryControllers) ? null : (
            <BodyText>Refresh the node information to check device access.</BodyText>
          )}
          <ControllerAccessCard
            page={controllers}
            controllerIdentityFingerprint={controllerIdentity}
            availableRequests={available}
            busy={blocked}
            onLoad={() => void read(Bindings.RemoteNodeQuery.Controllers.new({ after: undefined }))}
            onLoadMore={
              controllers?.next === undefined || controllers.identities.length >= maximumEntries
                ? undefined
                : () =>
                    void read(Bindings.RemoteNodeQuery.Controllers.new({ after: controllers.next }))
            }
            onChange={(next) => void change(next)}
          />
        </>
      ) : null}
    </>
  );
}

/** Reject overlap/non-progress; never accumulate an unbounded device inventory. */
export function appendPage<T>(
  previous: { entries: T[]; next?: Uint8Array | undefined } | undefined,
  page: { entries: T[]; next?: Uint8Array | undefined },
  id: (entry: T) => Uint8Array,
) {
  const entries = [...(previous?.entries ?? []), ...page.entries];
  const keys = entries.map((entry) => formatBytes(id(entry)));
  if (
    (page.entries.length === 0 && page.next !== undefined) ||
    entries.length > maximumEntries ||
    new Set(keys).size !== keys.length ||
    (previous?.next !== undefined &&
      page.next !== undefined &&
      formatBytes(previous.next) === formatBytes(page.next))
  ) {
    throw new Error("The inventory did not advance within its bound.");
  }
  return { entries, next: page.next };
}

/** The native last-change slot can move to another target while this route is blurred. */
export function reconcileRemoteChange(
  local: Bindings.RemoteChangeOperation | undefined,
  latest: Bindings.RemoteChangeOperation | undefined,
): Bindings.RemoteChangeOperation | undefined {
  if (
    local?.status.tag === Bindings.RemoteChangeStatus_Tags.Pending &&
    latest !== undefined &&
    latest.operationId > local.operationId
  ) {
    return {
      ...local,
      status: Bindings.RemoteChangeStatus.OutcomeUnknown.new({
        reason: Bindings.RemoteControlAnnounceUnknownReason.DeliveryUnconfirmed,
      }),
    };
  }
  return local;
}
