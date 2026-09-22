import * as Bindings from "@prns-internal/expo";
import { useState } from "react";

import { Badge, BodyText, Button, Card, CardStack, KeyValue, Subheading } from "@/ui/primitives";
import { TextField } from "@/ui/text-field";
import { formatBytes } from "../format";
import { ChoiceField } from "./choice-field";
import { ConfirmAction } from "./confirm-action";
import {
  connectionLabel,
  interfaceKindLabel,
  interfaceModes,
  loraRegions,
  radioSummary,
} from "./interface-format";
import { LoRaEditor } from "./lora-editor";
import type { RemoteChangeHandler } from "./node-controls";

export type RemoteInterfaceCardProps = {
  readonly entry: Bindings.RemoteInterfaceEntry;
  readonly details?: Bindings.RemoteInterfaceDetails | undefined;
  readonly peers?: Bindings.RemotePeerPage | undefined;
  readonly availableRequests: readonly Bindings.RemoteControlRequestKind[];
  readonly busy: boolean;
  readonly onLoadDetails: () => void;
  readonly onLoadPeers: () => void;
  readonly onLoadMorePeers?: (() => void) | undefined;
  readonly onChange: RemoteChangeHandler;
};

export function RemoteInterfaceCard({
  entry,
  details,
  peers,
  availableRequests,
  busy,
  onLoadDetails,
  onLoadPeers,
  onLoadMorePeers,
  onChange,
}: RemoteInterfaceCardProps) {
  const [editing, setEditing] = useState(false);
  const [showActivity, setShowActivity] = useState(false);
  const allows = (kind: Bindings.RemoteControlRequestKind) => availableRequests.includes(kind);
  const card =
    details?.configuration.tag === Bindings.RemoteInterfaceConfiguration_Tags.Available
      ? details.configuration.inner.card
      : undefined;
  const groups =
    details?.discoveryGroups.tag === Bindings.RemoteDiscoveryGroups_Tags.Available
      ? details.discoveryGroups.inner.groups
      : undefined;
  // Discovery-group inventory addresses only compatible supervisors. A valid
  // LoRa/USB interface can be unknown to that operation without disappearing.
  const unknown =
    details?.configuration.tag === Bindings.RemoteInterfaceConfiguration_Tags.UnknownInterface;
  const groupCompatible = supportsInterfaceGroups(entry.kind, groups);
  const canInspect =
    allows(Bindings.RemoteControlRequestKind.InventoryInterfaceConfig) ||
    allows(Bindings.RemoteControlRequestKind.InventoryInterfaceDiscoveryGroups);
  const canConfigure =
    allows(Bindings.RemoteControlRequestKind.SetInterfaceMode) ||
    (card !== undefined &&
      groupCompatible &&
      allows(Bindings.RemoteControlRequestKind.SetInterfaceGroup)) ||
    ((entry.kind === "lora" || card?.loraProfile !== undefined) &&
      allows(Bindings.RemoteControlRequestKind.SetInterfaceLoRaProfile)) ||
    (groups !== undefined &&
      allows(Bindings.RemoteControlRequestKind.ReplaceInterfaceDiscoveryGroups));
  const change = (next: Bindings.RemoteNodeChange) => {
    setEditing(false);
    onChange(next);
  };

  return (
    <Card>
      <Subheading>{card?.name || interfaceKindLabel(entry.kind)}</Subheading>
      <Badge>{connectionLabel(entry.connection)}</Badge>
      <KeyValue label="Power" value={entry.enabled ? "On" : "Off"} />
      <KeyValue
        label="Mode"
        value={interfaceModes.find((item) => item.value === entry.mode)?.label ?? "Not reported"}
      />
      {unknown ? (
        <BodyText>This interface is no longer available. Refresh the node's interfaces.</BodyText>
      ) : null}
      {card === undefined ? null : <InterfaceDetails card={card} />}
      {groups === undefined ? null : (
        <KeyValue
          label="Discovery groups"
          value={groups.length === 0 ? "None" : groups.join(", ")}
        />
      )}
      {canInspect ? (
        <Button disabled={busy} tone="secondary" onPress={onLoadDetails}>
          {details === undefined ? "Load interface settings" : "Refresh interface settings"}
        </Button>
      ) : null}
      {allows(Bindings.RemoteControlRequestKind.InventoryInterfacePeers) ? (
        <Button disabled={busy} tone="secondary" onPress={onLoadPeers}>
          {peers === undefined ? "View connected peers" : "Refresh peers"}
        </Button>
      ) : null}
      {peers === undefined ? null : (
        <PeerList page={peers} busy={busy} onLoadMore={onLoadMorePeers} />
      )}
      {!unknown && allows(Bindings.RemoteControlRequestKind.SetInterfacePower) ? (
        entry.enabled ? (
          <ConfirmAction
            label="Turn interface off"
            warning="If this interface carries the app's connection, turning it off will disconnect you. Make sure you can use another connection or the node's controls to turn it back on."
            confirmationLabel="Turn off"
            disabled={busy}
            onConfirm={() =>
              change(
                Bindings.RemoteNodeChange.InterfacePower.new({
                  interfaceId: entry.interfaceId,
                  enabled: false,
                }),
              )
            }
          />
        ) : (
          <Button
            disabled={busy}
            tone="secondary"
            onPress={() =>
              change(
                Bindings.RemoteNodeChange.InterfacePower.new({
                  interfaceId: entry.interfaceId,
                  enabled: true,
                }),
              )
            }
          >
            Turn interface on
          </Button>
        )
      ) : null}
      {!unknown &&
      entry.kind === "auto-wifi" &&
      allows(Bindings.RemoteControlRequestKind.SetStationUplink) ? (
        <>
          <ConfirmAction
            label="Enable Wi-Fi connection"
            warning="The node will try its saved Wi-Fi network. This may interrupt the current connection. Keep another way to reach it available."
            disabled={busy}
            onConfirm={() =>
              change(
                Bindings.RemoteNodeChange.StationUplink.new({
                  interfaceId: entry.interfaceId,
                  enabled: true,
                }),
              )
            }
          />
          <ConfirmAction
            label="Disable Wi-Fi connection"
            warning="If the app reaches this node over Wi-Fi, it may disconnect. Use Bluetooth or another connection to turn Wi-Fi back on."
            disabled={busy}
            onConfirm={() =>
              change(
                Bindings.RemoteNodeChange.StationUplink.new({
                  interfaceId: entry.interfaceId,
                  enabled: false,
                }),
              )
            }
          />
        </>
      ) : null}
      {!unknown && canConfigure ? (
        <Button
          disabled={busy}
          tone="secondary"
          onPress={() => setEditing((previous) => !previous)}
        >
          {editing ? "Close settings editor" : "Edit interface settings"}
        </Button>
      ) : null}
      {editing && !unknown ? (
        <InterfaceEditor
          entry={entry}
          card={card}
          groups={groups}
          availableRequests={availableRequests}
          busy={busy}
          onChange={change}
        />
      ) : null}
      <Button
        tone="secondary"
        accessibilityState={{ expanded: showActivity }}
        onPress={() => setShowActivity((previous) => !previous)}
      >
        {showActivity ? "Hide connection details" : "Show connection details"}
      </Button>
      {showActivity ? (
        <>
          <KeyValue label="Interface ID" value={formatBytes(entry.interfaceId)} />
          <KeyValue
            label="Data sent / received"
            value={`${entry.txBytes.toString()} / ${entry.rxBytes.toString()} bytes`}
          />
          <KeyValue label="Active links" value={entry.links.toString()} />
          <KeyValue label="Transfer rate" value={`${entry.rateBytesPerSec} bytes/s`} />
        </>
      ) : null}
    </Card>
  );
}

function InterfaceDetails({ card }: { readonly card: Bindings.RemoteInterfaceCard }) {
  const profile = card.loraProfile;
  return (
    <>
      {card.group ? <KeyValue label="Interface group" value={card.group} /> : null}
      {card.failure ? <BodyText>This interface needs attention: {card.failure}</BodyText> : null}
      <KeyValue label="Known destinations" value={card.destinations.toString()} />
      <KeyValue label="Forwarded links" value={card.transportedLinks.toString()} />
      {profile === undefined ? (
        card.configuration ? (
          <KeyValue label="Configuration" value={card.configuration} />
        ) : null
      ) : (
        <>
          <KeyValue
            label="LoRa region"
            value={
              loraRegions.find((region) => region.value === profile.region)?.label ?? "Not reported"
            }
          />
          <KeyValue label="Frequency" value={`${profile.frequencyHz / 1_000_000} MHz`} />
          <KeyValue label="Bandwidth" value={`${profile.bandwidthHz / 1_000} kHz`} />
          <KeyValue
            label="Spreading factor / coding rate"
            value={`SF${profile.spreadingFactor} · 4/${profile.codingRate}`}
          />
          <KeyValue label="Transmit power" value={`${profile.txPowerDbm} dBm`} />
          <KeyValue label="Preamble" value={`${profile.preambleSymbols} symbols`} />
        </>
      )}
    </>
  );
}

function InterfaceEditor({
  entry,
  card,
  groups,
  availableRequests,
  busy,
  onChange,
}: {
  readonly entry: Bindings.RemoteInterfaceEntry;
  readonly card: Bindings.RemoteInterfaceCard | undefined;
  readonly groups: readonly string[] | undefined;
  readonly availableRequests: readonly Bindings.RemoteControlRequestKind[];
  readonly busy: boolean;
  readonly onChange: RemoteChangeHandler;
}) {
  const [mode, setMode] = useState(entry.mode);
  const [group, setGroup] = useState(card?.group ?? "");
  const [groupNames, setGroupNames] = useState(groups?.join("\n") ?? "");
  const allows = (kind: Bindings.RemoteControlRequestKind) => availableRequests.includes(kind);
  return (
    <CardStack>
      {allows(Bindings.RemoteControlRequestKind.SetInterfaceMode) ? (
        <>
          <ChoiceField
            label="Interface mode"
            options={interfaceModes}
            value={mode}
            disabled={busy}
            onChange={setMode}
          />
          <ConfirmAction
            label="Save interface mode"
            warning="Changing the interface mode may affect which nodes can connect or exchange traffic. Keep another way to reach this node available."
            disabled={busy}
            onConfirm={() =>
              onChange(
                Bindings.RemoteNodeChange.InterfaceMode.new({
                  interfaceId: entry.interfaceId,
                  mode,
                }),
              )
            }
          />
        </>
      ) : null}
      {card !== undefined &&
      supportsInterfaceGroups(entry.kind, groups) &&
      allows(Bindings.RemoteControlRequestKind.SetInterfaceGroup) ? (
        <>
          <TextField
            label="Interface group"
            value={group}
            onChangeText={setGroup}
            editable={!busy}
            autoCapitalize="none"
            autoCorrect={false}
          />
          <ConfirmAction
            label="Save interface group"
            warning="Changing the interface group may disconnect peers. Keep another way to reach this node available."
            disabled={busy}
            onConfirm={() =>
              onChange(
                Bindings.RemoteNodeChange.InterfaceGroup.new({
                  interfaceId: entry.interfaceId,
                  group,
                }),
              )
            }
          />
        </>
      ) : null}
      {groups !== undefined &&
      allows(Bindings.RemoteControlRequestKind.ReplaceInterfaceDiscoveryGroups) ? (
        <>
          <TextField
            label="Discovery groups (one per line)"
            value={groupNames}
            onChangeText={setGroupNames}
            editable={!busy}
            autoCapitalize="none"
            autoCorrect={false}
            multiline
          />
          <BodyText muted>Nearby nodes need a group in common to discover each other.</BodyText>
          <ConfirmAction
            label="Save discovery groups"
            warning="Replacing discovery groups may disconnect nearby peers. Keep another connection available so you can restore them."
            disabled={busy}
            onConfirm={() =>
              onChange(
                Bindings.RemoteNodeChange.DiscoveryGroups.new({
                  interfaceId: entry.interfaceId,
                  groups: groupNames === "" ? [] : groupNames.split("\n"),
                }),
              )
            }
          />
        </>
      ) : null}
      {(entry.kind === "lora" || card?.loraProfile !== undefined) &&
      allows(Bindings.RemoteControlRequestKind.SetInterfaceLoRaProfile) ? (
        <LoRaEditor
          profile={card?.loraProfile}
          busy={busy}
          onSave={(profile) =>
            onChange(
              Bindings.RemoteNodeChange.InterfaceLoRa.new({
                interfaceId: entry.interfaceId,
                profile,
              }),
            )
          }
        />
      ) : null}
    </CardStack>
  );
}

function supportsInterfaceGroups(kind: string, groups: readonly string[] | undefined): boolean {
  return groups !== undefined || kind === "bluetooth-auto" || kind === "auto-wifi";
}

function PeerList({
  page,
  busy,
  onLoadMore,
}: {
  readonly page: Bindings.RemotePeerPage;
  readonly busy: boolean;
  readonly onLoadMore: (() => void) | undefined;
}) {
  return (
    <CardStack>
      <Subheading>Peers</Subheading>
      {page.entries.length === 0 ? <BodyText>No connected peers were reported.</BodyText> : null}
      {page.entries.map((peer) => {
        const signal = radioSummary(peer.radio);
        return (
          <Card key={formatBytes(peer.peerId)}>
            <KeyValue label="Peer ID" value={formatBytes(peer.peerId)} />
            <Badge>{connectionLabel(peer.connection)}</Badge>
            {peer.details ? <BodyText>{peer.details}</BodyText> : null}
            <KeyValue
              label="Data sent / received"
              value={`${peer.txBytes.toString()} / ${peer.rxBytes.toString()} bytes`}
            />
            <KeyValue
              label="Active links / known destinations"
              value={`${peer.links} / ${peer.destinations}`}
            />
            <KeyValue label="Transfer rate" value={`${peer.rateBytesPerSec} bytes/s`} />
            {signal === null ? null : <KeyValue label="Signal" value={signal} />}
          </Card>
        );
      })}
      {page.next !== undefined && onLoadMore !== undefined ? (
        <Button disabled={busy} tone="secondary" onPress={onLoadMore}>
          Load more peers
        </Button>
      ) : null}
    </CardStack>
  );
}
