// Generated from public prns-core Remote Control declarations. Do not edit.
import type * as C from "personal-rns/remote-control";
import * as N from "./prns_host_uniffi";
import { lowerIdentityConfig } from "./host-adapter.generated";
function unexpected(value: never): never { throw new TypeError(`unknown Remote Control case: ${String(value)}`); }
export function lowerRemoteControlRequest(value: C.RemoteControlRequest): N.RemoteControlRequest {
  switch (value.tag) {
    case "Describe": return N.RemoteControlRequest.Describe.new();
    case "AnnounceSelf": return N.RemoteControlRequest.AnnounceSelf.new();
    case "InventoryInterfaces": return N.RemoteControlRequest.InventoryInterfaces.new({ page: lowerRemoteControlInterfacePage(value.data.page) });
    case "SetInterfacePower": return N.RemoteControlRequest.SetInterfacePower.new({ id: lowerRemoteControlInterfaceId(value.data.id), power: lowerRemoteControlInterfacePower(value.data.power) });
    case "SetInterfaceMode": return N.RemoteControlRequest.SetInterfaceMode.new({ id: lowerRemoteControlInterfaceId(value.data.id), mode: lowerRemoteControlInterfaceMode(value.data.mode) });
    case "SetInterfaceGroup": return N.RemoteControlRequest.SetInterfaceGroup.new({ id: lowerRemoteControlInterfaceId(value.data.id), group: lowerRemoteControlInterfaceGroup(value.data.group) });
    case "InventoryInterfaceDiscoveryGroups": return N.RemoteControlRequest.InventoryInterfaceDiscoveryGroups.new({ id: lowerRemoteControlInterfaceId(value.data.id) });
    case "ReplaceInterfaceDiscoveryGroups": return N.RemoteControlRequest.ReplaceInterfaceDiscoveryGroups.new({ id: lowerRemoteControlInterfaceId(value.data.id), groups: lowerRemoteControlDiscoveryGroups(value.data.groups) });
    case "InventoryInterfacePeers": return N.RemoteControlRequest.InventoryInterfacePeers.new({ id: lowerRemoteControlInterfaceId(value.data.id), page: lowerRemoteControlPeerPage(value.data.page) });
    case "InventoryInterfaceConfig": return N.RemoteControlRequest.InventoryInterfaceConfig.new({ id: lowerRemoteControlInterfaceId(value.data.id) });
    case "SetInterfaceLoRaProfile": return N.RemoteControlRequest.SetInterfaceLoRaProfile.new({ id: lowerRemoteControlInterfaceId(value.data.id), profile: lowerRemoteControlLoRaProfile(value.data.profile) });
    case "SetInterfaceWifiStation": return N.RemoteControlRequest.SetInterfaceWifiStation.new({ id: lowerRemoteControlInterfaceId(value.data.id), station: lowerRemoteControlWifiStation(value.data.station) });
    case "InventoryControllers": return N.RemoteControlRequest.InventoryControllers.new({ page: lowerRemoteControlControllerPage(value.data.page) });
    case "AuthorizeController": return N.RemoteControlRequest.AuthorizeController.new({ controller: lowerRemoteControlControllerIdentity(value.data.controller), permittedRequests: lowerRemoteControlRequestSet(value.data.permittedRequests) });
    case "RevokeController": return N.RemoteControlRequest.RevokeController.new({ hash: lowerRemoteControlIdentityHash(value.data.hash) });
    case "DescribeBuild": return N.RemoteControlRequest.DescribeBuild.new();
    case "DescribePower": return N.RemoteControlRequest.DescribePower.new();
    case "SleepRadios": return N.RemoteControlRequest.SleepRadios.new();
    case "WakeRadios": return N.RemoteControlRequest.WakeRadios.new();
    case "SetSystemPower": return N.RemoteControlRequest.SetSystemPower.new({ power: lowerRemoteControlSystemPower(value.data.power) });
    case "SetGnssPower": return N.RemoteControlRequest.SetGnssPower.new({ power: lowerRemoteControlGnssPower(value.data.power) });
    case "SetDisplayVisibility": return N.RemoteControlRequest.SetDisplayVisibility.new({ visibility: lowerRemoteControlDisplayVisibility(value.data.visibility) });
    case "SetDisplayAutoOff": return N.RemoteControlRequest.SetDisplayAutoOff.new({ autoOff: lowerRemoteControlDisplayAutoOff(value.data.autoOff) });
    case "SetStationUplink": return N.RemoteControlRequest.SetStationUplink.new({ id: lowerRemoteControlInterfaceId(value.data.id), uplink: lowerRemoteControlStationUplink(value.data.uplink) });
    case "SetEspRadioMode": return N.RemoteControlRequest.SetEspRadioMode.new({ mode: lowerRemoteControlEspRadioMode(value.data.mode) });
    case "StageWifiCredentials": return N.RemoteControlRequest.StageWifiCredentials.new({ station: lowerRemoteControlWifiStation(value.data.station) });
    case "ActivateWifiCredentials": return N.RemoteControlRequest.ActivateWifiCredentials.new({ revision: lowerRemoteControlWifiCredentialRevision(value.data.revision) });
    case "ConfirmWifiCredentials": return N.RemoteControlRequest.ConfirmWifiCredentials.new({ revision: lowerRemoteControlWifiCredentialRevision(value.data.revision) });
    case "CancelWifiCredentials": return N.RemoteControlRequest.CancelWifiCredentials.new({ revision: lowerRemoteControlWifiCredentialRevision(value.data.revision) });
    case "InspectWifiTransaction": return N.RemoteControlRequest.InspectWifiTransaction.new();
    default: return unexpected(value);
  }
}
export function lowerRemoteControlInterfacePage(value: C.RemoteControlInterfacePage): N.RemoteControlInterfacePage {
  switch (value.tag) {
    case "First": return N.RemoteControlInterfacePage.First.new();
    case "After": return N.RemoteControlInterfacePage.After.new({ value: lowerRemoteControlInterfaceCursor(value.data.value) });
    default: return unexpected(value);
  }
}
export function lowerRemoteControlInterfaceCursor(value: C.RemoteControlInterfaceCursor): N.RemoteControlInterfaceCursor {
  return { value: lowerRemoteControlInterfaceId(value.value) };
}
export function liftRemoteControlInterfaceCursor(value: N.RemoteControlInterfaceCursor): C.RemoteControlInterfaceCursor {
  return { value: liftRemoteControlInterfaceId(value.value) };
}
export function lowerRemoteControlInterfaceId(value: C.RemoteControlInterfaceId): N.RemoteControlInterfaceId {
  return { value: value.value };
}
export function liftRemoteControlInterfaceId(value: N.RemoteControlInterfaceId): C.RemoteControlInterfaceId {
  return { value: value.value };
}
export function lowerRemoteControlInterfacePower(value: C.RemoteControlInterfacePower): N.RemoteControlInterfacePower {
  switch (value.tag) {
    case "Off": return N.RemoteControlInterfacePower.Off;
    case "On": return N.RemoteControlInterfacePower.On;
    default: return unexpected(value);
  }
}
export function lowerRemoteControlInterfaceMode(value: C.RemoteControlInterfaceMode): N.RemoteControlInterfaceMode {
  switch (value.tag) {
    case "Full": return N.RemoteControlInterfaceMode.Full;
    case "PointToPoint": return N.RemoteControlInterfaceMode.PointToPoint;
    case "AccessPoint": return N.RemoteControlInterfaceMode.AccessPoint;
    case "Roaming": return N.RemoteControlInterfaceMode.Roaming;
    case "Boundary": return N.RemoteControlInterfaceMode.Boundary;
    case "Gateway": return N.RemoteControlInterfaceMode.Gateway;
    case "Internal": return N.RemoteControlInterfaceMode.Internal;
    default: return unexpected(value);
  }
}
export function liftRemoteControlInterfaceMode(value: N.RemoteControlInterfaceMode): C.RemoteControlInterfaceMode {
  switch (value) {
    case N.RemoteControlInterfaceMode.Full: return { tag: "Full" };
    case N.RemoteControlInterfaceMode.PointToPoint: return { tag: "PointToPoint" };
    case N.RemoteControlInterfaceMode.AccessPoint: return { tag: "AccessPoint" };
    case N.RemoteControlInterfaceMode.Roaming: return { tag: "Roaming" };
    case N.RemoteControlInterfaceMode.Boundary: return { tag: "Boundary" };
    case N.RemoteControlInterfaceMode.Gateway: return { tag: "Gateway" };
    case N.RemoteControlInterfaceMode.Internal: return { tag: "Internal" };
    default: return unexpected(value);
  }
}
export function lowerRemoteControlInterfaceGroup(value: C.RemoteControlInterfaceGroup): N.RemoteControlInterfaceGroup {
  return { value: value.value };
}
export function lowerRemoteControlDiscoveryGroups(value: C.RemoteControlDiscoveryGroups): N.RemoteControlDiscoveryGroups {
  return { groups: value.groups.map(item => item) };
}
export function liftRemoteControlDiscoveryGroups(value: N.RemoteControlDiscoveryGroups): C.RemoteControlDiscoveryGroups {
  return { groups: value.groups.map(item => item) };
}
export function lowerRemoteControlPeerPage(value: C.RemoteControlPeerPage): N.RemoteControlPeerPage {
  switch (value.tag) {
    case "First": return N.RemoteControlPeerPage.First.new();
    case "After": return N.RemoteControlPeerPage.After.new({ value: lowerRemoteControlPeerCursor(value.data.value) });
    default: return unexpected(value);
  }
}
export function lowerRemoteControlPeerCursor(value: C.RemoteControlPeerCursor): N.RemoteControlPeerCursor {
  return { value: lowerRemoteControlInterfaceId(value.value) };
}
export function liftRemoteControlPeerCursor(value: N.RemoteControlPeerCursor): C.RemoteControlPeerCursor {
  return { value: liftRemoteControlInterfaceId(value.value) };
}
export function lowerRemoteControlLoRaProfile(value: C.RemoteControlLoRaProfile): N.RemoteControlLoRaProfile {
  return { value: value.value };
}
export function lowerRemoteControlWifiStation(value: C.RemoteControlWifiStation): N.RemoteControlWifiStation {
  return { ssid: value.ssid, password: value.password };
}
export function lowerRemoteControlControllerPage(value: C.RemoteControlControllerPage): N.RemoteControlControllerPage {
  switch (value.tag) {
    case "First": return N.RemoteControlControllerPage.First.new();
    case "After": return N.RemoteControlControllerPage.After.new({ value: lowerRemoteControlControllerCursor(value.data.value) });
    default: return unexpected(value);
  }
}
export function lowerRemoteControlControllerCursor(value: C.RemoteControlControllerCursor): N.RemoteControlControllerCursor {
  return { value: lowerRemoteControlIdentityHash(value.value) };
}
export function liftRemoteControlControllerCursor(value: N.RemoteControlControllerCursor): C.RemoteControlControllerCursor {
  return { value: liftRemoteControlIdentityHash(value.value) };
}
export function lowerRemoteControlIdentityHash(value: C.RemoteControlIdentityHash): N.RemoteControlIdentityHash {
  return { value: value.value };
}
export function liftRemoteControlIdentityHash(value: N.RemoteControlIdentityHash): C.RemoteControlIdentityHash {
  return { value: value.value };
}
export function lowerRemoteControlControllerIdentity(value: C.RemoteControlControllerIdentity): N.RemoteControlControllerIdentity {
  return { publicKeys: value.publicKeys };
}
export function liftRemoteControlControllerIdentity(value: N.RemoteControlControllerIdentity): C.RemoteControlControllerIdentity {
  return { publicKeys: value.publicKeys };
}
export function lowerRemoteControlRequestSet(value: C.RemoteControlRequestSet): N.RemoteControlRequestSet {
  return { kinds: value.kinds.map(item => lowerRemoteControlRequestKind(item)) };
}
export function liftRemoteControlRequestSet(value: N.RemoteControlRequestSet): C.RemoteControlRequestSet {
  return { kinds: value.kinds.map(item => liftRemoteControlRequestKind(item)) };
}
export function lowerRemoteControlRequestKind(value: C.RemoteControlRequestKind): N.RemoteControlRequestKind {
  switch (value.tag) {
    case "Describe": return N.RemoteControlRequestKind.Describe;
    case "AnnounceSelf": return N.RemoteControlRequestKind.AnnounceSelf;
    case "InventoryInterfaces": return N.RemoteControlRequestKind.InventoryInterfaces;
    case "SetInterfacePower": return N.RemoteControlRequestKind.SetInterfacePower;
    case "SleepRadios": return N.RemoteControlRequestKind.SleepRadios;
    case "WakeRadios": return N.RemoteControlRequestKind.WakeRadios;
    case "SetInterfaceMode": return N.RemoteControlRequestKind.SetInterfaceMode;
    case "SetInterfaceGroup": return N.RemoteControlRequestKind.SetInterfaceGroup;
    case "InventoryInterfacePeers": return N.RemoteControlRequestKind.InventoryInterfacePeers;
    case "InventoryInterfaceConfig": return N.RemoteControlRequestKind.InventoryInterfaceConfig;
    case "SetInterfaceLoRaProfile": return N.RemoteControlRequestKind.SetInterfaceLoRaProfile;
    case "DescribeBuild": return N.RemoteControlRequestKind.DescribeBuild;
    case "SetInterfaceWifiStation": return N.RemoteControlRequestKind.SetInterfaceWifiStation;
    case "InventoryControllers": return N.RemoteControlRequestKind.InventoryControllers;
    case "AuthorizeController": return N.RemoteControlRequestKind.AuthorizeController;
    case "RevokeController": return N.RemoteControlRequestKind.RevokeController;
    case "DescribePower": return N.RemoteControlRequestKind.DescribePower;
    case "SetSystemPower": return N.RemoteControlRequestKind.SetSystemPower;
    case "SetGnssPower": return N.RemoteControlRequestKind.SetGnssPower;
    case "SetDisplayVisibility": return N.RemoteControlRequestKind.SetDisplayVisibility;
    case "SetDisplayAutoOff": return N.RemoteControlRequestKind.SetDisplayAutoOff;
    case "SetStationUplink": return N.RemoteControlRequestKind.SetStationUplink;
    case "SetEspRadioMode": return N.RemoteControlRequestKind.SetEspRadioMode;
    case "StageWifiCredentials": return N.RemoteControlRequestKind.StageWifiCredentials;
    case "ActivateWifiCredentials": return N.RemoteControlRequestKind.ActivateWifiCredentials;
    case "ConfirmWifiCredentials": return N.RemoteControlRequestKind.ConfirmWifiCredentials;
    case "CancelWifiCredentials": return N.RemoteControlRequestKind.CancelWifiCredentials;
    case "InspectWifiTransaction": return N.RemoteControlRequestKind.InspectWifiTransaction;
    case "InventoryInterfaceDiscoveryGroups": return N.RemoteControlRequestKind.InventoryInterfaceDiscoveryGroups;
    case "ReplaceInterfaceDiscoveryGroups": return N.RemoteControlRequestKind.ReplaceInterfaceDiscoveryGroups;
    default: return unexpected(value);
  }
}
export function liftRemoteControlRequestKind(value: N.RemoteControlRequestKind): C.RemoteControlRequestKind {
  switch (value) {
    case N.RemoteControlRequestKind.Describe: return { tag: "Describe" };
    case N.RemoteControlRequestKind.AnnounceSelf: return { tag: "AnnounceSelf" };
    case N.RemoteControlRequestKind.InventoryInterfaces: return { tag: "InventoryInterfaces" };
    case N.RemoteControlRequestKind.SetInterfacePower: return { tag: "SetInterfacePower" };
    case N.RemoteControlRequestKind.SleepRadios: return { tag: "SleepRadios" };
    case N.RemoteControlRequestKind.WakeRadios: return { tag: "WakeRadios" };
    case N.RemoteControlRequestKind.SetInterfaceMode: return { tag: "SetInterfaceMode" };
    case N.RemoteControlRequestKind.SetInterfaceGroup: return { tag: "SetInterfaceGroup" };
    case N.RemoteControlRequestKind.InventoryInterfacePeers: return { tag: "InventoryInterfacePeers" };
    case N.RemoteControlRequestKind.InventoryInterfaceConfig: return { tag: "InventoryInterfaceConfig" };
    case N.RemoteControlRequestKind.SetInterfaceLoRaProfile: return { tag: "SetInterfaceLoRaProfile" };
    case N.RemoteControlRequestKind.DescribeBuild: return { tag: "DescribeBuild" };
    case N.RemoteControlRequestKind.SetInterfaceWifiStation: return { tag: "SetInterfaceWifiStation" };
    case N.RemoteControlRequestKind.InventoryControllers: return { tag: "InventoryControllers" };
    case N.RemoteControlRequestKind.AuthorizeController: return { tag: "AuthorizeController" };
    case N.RemoteControlRequestKind.RevokeController: return { tag: "RevokeController" };
    case N.RemoteControlRequestKind.DescribePower: return { tag: "DescribePower" };
    case N.RemoteControlRequestKind.SetSystemPower: return { tag: "SetSystemPower" };
    case N.RemoteControlRequestKind.SetGnssPower: return { tag: "SetGnssPower" };
    case N.RemoteControlRequestKind.SetDisplayVisibility: return { tag: "SetDisplayVisibility" };
    case N.RemoteControlRequestKind.SetDisplayAutoOff: return { tag: "SetDisplayAutoOff" };
    case N.RemoteControlRequestKind.SetStationUplink: return { tag: "SetStationUplink" };
    case N.RemoteControlRequestKind.SetEspRadioMode: return { tag: "SetEspRadioMode" };
    case N.RemoteControlRequestKind.StageWifiCredentials: return { tag: "StageWifiCredentials" };
    case N.RemoteControlRequestKind.ActivateWifiCredentials: return { tag: "ActivateWifiCredentials" };
    case N.RemoteControlRequestKind.ConfirmWifiCredentials: return { tag: "ConfirmWifiCredentials" };
    case N.RemoteControlRequestKind.CancelWifiCredentials: return { tag: "CancelWifiCredentials" };
    case N.RemoteControlRequestKind.InspectWifiTransaction: return { tag: "InspectWifiTransaction" };
    case N.RemoteControlRequestKind.InventoryInterfaceDiscoveryGroups: return { tag: "InventoryInterfaceDiscoveryGroups" };
    case N.RemoteControlRequestKind.ReplaceInterfaceDiscoveryGroups: return { tag: "ReplaceInterfaceDiscoveryGroups" };
    default: return unexpected(value);
  }
}
export function lowerRemoteControlSystemPower(value: C.RemoteControlSystemPower): N.RemoteControlSystemPower {
  switch (value.tag) {
    case "Awake": return N.RemoteControlSystemPower.Awake;
    case "Asleep": return N.RemoteControlSystemPower.Asleep;
    default: return unexpected(value);
  }
}
export function lowerRemoteControlGnssPower(value: C.RemoteControlGnssPower): N.RemoteControlGnssPower {
  switch (value.tag) {
    case "Off": return N.RemoteControlGnssPower.Off;
    case "On": return N.RemoteControlGnssPower.On;
    default: return unexpected(value);
  }
}
export function lowerRemoteControlDisplayVisibility(value: C.RemoteControlDisplayVisibility): N.RemoteControlDisplayVisibility {
  switch (value.tag) {
    case "Hidden": return N.RemoteControlDisplayVisibility.Hidden;
    case "Visible": return N.RemoteControlDisplayVisibility.Visible;
    default: return unexpected(value);
  }
}
export function lowerRemoteControlDisplayAutoOff(value: C.RemoteControlDisplayAutoOff): N.RemoteControlDisplayAutoOff {
  switch (value.tag) {
    case "Disabled": return N.RemoteControlDisplayAutoOff.Disabled;
    case "Enabled": return N.RemoteControlDisplayAutoOff.Enabled;
    default: return unexpected(value);
  }
}
export function lowerRemoteControlStationUplink(value: C.RemoteControlStationUplink): N.RemoteControlStationUplink {
  switch (value.tag) {
    case "Disabled": return N.RemoteControlStationUplink.Disabled;
    case "Enabled": return N.RemoteControlStationUplink.Enabled;
    default: return unexpected(value);
  }
}
export function lowerRemoteControlEspRadioMode(value: C.RemoteControlEspRadioMode): N.RemoteControlEspRadioMode {
  switch (value.tag) {
    case "Bluetooth": return N.RemoteControlEspRadioMode.Bluetooth;
    case "AccessPoint": return N.RemoteControlEspRadioMode.AccessPoint;
    default: return unexpected(value);
  }
}
export function lowerRemoteControlWifiCredentialRevision(value: C.RemoteControlWifiCredentialRevision): N.RemoteControlWifiCredentialRevision {
  return { value: value.value };
}
export function liftRemoteControlWifiCredentialRevision(value: N.RemoteControlWifiCredentialRevision): C.RemoteControlWifiCredentialRevision {
  return { value: value.value };
}
export function liftRemoteControlResponse(value: N.RemoteControlResponse): C.RemoteControlResponse {
  switch (value.tag) {
    case N.RemoteControlResponse_Tags.Describe: return { tag: "Describe", data: { value: liftRemoteControlDescription(value.inner.value) } };
    case N.RemoteControlResponse_Tags.AnnounceSelf: return { tag: "AnnounceSelf", data: { value: liftRemoteControlAnnounceSelfOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.InventoryInterfaces: return { tag: "InventoryInterfaces", data: { value: liftRemoteControlInterfaceInventory(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetInterfacePower: return { tag: "SetInterfacePower", data: { value: liftRemoteControlPowerOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetInterfaceMode: return { tag: "SetInterfaceMode", data: { value: liftRemoteControlModeOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetInterfaceGroup: return { tag: "SetInterfaceGroup", data: { value: liftRemoteControlGroupOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.InventoryInterfaceDiscoveryGroups: return { tag: "InventoryInterfaceDiscoveryGroups", data: { value: liftRemoteControlDiscoveryGroupsInventoryOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.ReplaceInterfaceDiscoveryGroups: return { tag: "ReplaceInterfaceDiscoveryGroups", data: { value: liftRemoteControlDiscoveryGroupsReplaceOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.InventoryInterfacePeers: return { tag: "InventoryInterfacePeers", data: { value: liftRemoteControlInterfacePeersOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.InventoryInterfaceConfig: return { tag: "InventoryInterfaceConfig", data: { value: liftRemoteControlInterfaceConfigOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetInterfaceLoRaProfile: return { tag: "SetInterfaceLoRaProfile", data: { value: liftRemoteControlLoRaOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetInterfaceWifiStation: return { tag: "SetInterfaceWifiStation", data: { value: liftRemoteControlWifiStationOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.InventoryControllers: return { tag: "InventoryControllers", data: { value: liftRemoteControlControllerInventory(value.inner.value) } };
    case N.RemoteControlResponse_Tags.AuthorizeController: return { tag: "AuthorizeController", data: { value: liftRemoteControlAuthorizeControllerOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.RevokeController: return { tag: "RevokeController", data: { value: liftRemoteControlRevokeControllerOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.DescribeBuild: return { tag: "DescribeBuild", data: { value: liftRemoteControlBuildVersion(value.inner.value) } };
    case N.RemoteControlResponse_Tags.DescribePower: return { tag: "DescribePower", data: { value: liftRemoteControlPowerSnapshot(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SleepRadios: return { tag: "SleepRadios", data: { value: liftRemoteControlSleepOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.WakeRadios: return { tag: "WakeRadios", data: { value: liftRemoteControlSleepOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetSystemPower: return { tag: "SetSystemPower", data: { value: liftRemoteControlApplyOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetGnssPower: return { tag: "SetGnssPower", data: { value: liftRemoteControlApplyOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetDisplayVisibility: return { tag: "SetDisplayVisibility", data: { value: liftRemoteControlApplyOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetDisplayAutoOff: return { tag: "SetDisplayAutoOff", data: { value: liftRemoteControlApplyOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetStationUplink: return { tag: "SetStationUplink", data: { value: liftRemoteControlApplyOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.SetEspRadioMode: return { tag: "SetEspRadioMode", data: { value: liftRemoteControlApplyOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.StageWifiCredentials: return { tag: "StageWifiCredentials", data: { value: liftRemoteControlWifiStageOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.ActivateWifiCredentials: return { tag: "ActivateWifiCredentials", data: { value: liftRemoteControlApplyOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.ConfirmWifiCredentials: return { tag: "ConfirmWifiCredentials", data: { value: liftRemoteControlApplyOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.CancelWifiCredentials: return { tag: "CancelWifiCredentials", data: { value: liftRemoteControlApplyOutcome(value.inner.value) } };
    case N.RemoteControlResponse_Tags.InspectWifiTransaction: return { tag: "InspectWifiTransaction", data: { value: liftRemoteControlWifiTransactionStatus(value.inner.value) } };
    case N.RemoteControlResponse_Tags.ProtocolError: return { tag: "ProtocolError", data: { value: liftRemoteControlProtocolError(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlDescription(value: N.RemoteControlDescription): C.RemoteControlDescription {
  return { availableRequests: liftRemoteControlRequestSet(value.availableRequests) };
}
export function liftRemoteControlAnnounceSelfOutcome(value: N.RemoteControlAnnounceSelfOutcome): C.RemoteControlAnnounceSelfOutcome {
  switch (value) {
    case N.RemoteControlAnnounceSelfOutcome.Announced: return { tag: "Announced" };
    case N.RemoteControlAnnounceSelfOutcome.Unavailable: return { tag: "Unavailable" };
    case N.RemoteControlAnnounceSelfOutcome.Rejected: return { tag: "Rejected" };
    case N.RemoteControlAnnounceSelfOutcome.WriteFailed: return { tag: "WriteFailed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlInterfaceInventory(value: N.RemoteControlInterfaceInventory): C.RemoteControlInterfaceInventory {
  return { entries: value.entries.map(item => liftRemoteControlInterfaceEntry(item)), continuation: liftRemoteControlInterfaceContinuation(value.continuation) };
}
export function liftRemoteControlInterfaceEntry(value: N.RemoteControlInterfaceEntry): C.RemoteControlInterfaceEntry {
  return { id: liftRemoteControlInterfaceId(value.id), kind: liftRemoteControlInterfaceKind(value.kind), mode: liftRemoteControlInterfaceMode(value.mode), connection: liftRemoteControlConnectionState(value.connection), enabled: value.enabled, txBytes: value.txBytes, rxBytes: value.rxBytes, links: value.links, rateBytesPerSec: value.rateBytesPerSec };
}
export function liftRemoteControlInterfaceKind(value: N.RemoteControlInterfaceKind): C.RemoteControlInterfaceKind {
  switch (value) {
    case N.RemoteControlInterfaceKind.Loopback: return { tag: "Loopback" };
    case N.RemoteControlInterfaceKind.TcpClient: return { tag: "TcpClient" };
    case N.RemoteControlInterfaceKind.TcpServer: return { tag: "TcpServer" };
    case N.RemoteControlInterfaceKind.Udp: return { tag: "Udp" };
    case N.RemoteControlInterfaceKind.Serial: return { tag: "Serial" };
    case N.RemoteControlInterfaceKind.UsbAutoHost: return { tag: "UsbAutoHost" };
    case N.RemoteControlInterfaceKind.UsbAutoDevice: return { tag: "UsbAutoDevice" };
    case N.RemoteControlInterfaceKind.AutoWifi: return { tag: "AutoWifi" };
    case N.RemoteControlInterfaceKind.WifiPeer: return { tag: "WifiPeer" };
    case N.RemoteControlInterfaceKind.LocalServer: return { tag: "LocalServer" };
    case N.RemoteControlInterfaceKind.LocalClient: return { tag: "LocalClient" };
    case N.RemoteControlInterfaceKind.TcpServerPeer: return { tag: "TcpServerPeer" };
    case N.RemoteControlInterfaceKind.BluetoothAuto: return { tag: "BluetoothAuto" };
    case N.RemoteControlInterfaceKind.BluetoothPeer: return { tag: "BluetoothPeer" };
    case N.RemoteControlInterfaceKind.LoRa: return { tag: "LoRa" };
    case N.RemoteControlInterfaceKind.Kiss: return { tag: "Kiss" };
    case N.RemoteControlInterfaceKind.Ax25Kiss: return { tag: "Ax25Kiss" };
    case N.RemoteControlInterfaceKind.Pipe: return { tag: "Pipe" };
    case N.RemoteControlInterfaceKind.Rnode: return { tag: "Rnode" };
    case N.RemoteControlInterfaceKind.BackboneServer: return { tag: "BackboneServer" };
    case N.RemoteControlInterfaceKind.BackboneServerPeer: return { tag: "BackboneServerPeer" };
    case N.RemoteControlInterfaceKind.BackboneClient: return { tag: "BackboneClient" };
    case N.RemoteControlInterfaceKind.EspNow: return { tag: "EspNow" };
    case N.RemoteControlInterfaceKind.WebSocketClient: return { tag: "WebSocketClient" };
    case N.RemoteControlInterfaceKind.WebSocketServer: return { tag: "WebSocketServer" };
    case N.RemoteControlInterfaceKind.WebSocketServerPeer: return { tag: "WebSocketServerPeer" };
    case N.RemoteControlInterfaceKind.WifiDirect: return { tag: "WifiDirect" };
    case N.RemoteControlInterfaceKind.WifiDirectPeer: return { tag: "WifiDirectPeer" };
    case N.RemoteControlInterfaceKind.WifiAware: return { tag: "WifiAware" };
    case N.RemoteControlInterfaceKind.WifiAwarePeer: return { tag: "WifiAwarePeer" };
    case N.RemoteControlInterfaceKind.I2p: return { tag: "I2p" };
    case N.RemoteControlInterfaceKind.I2pPeer: return { tag: "I2pPeer" };
    case N.RemoteControlInterfaceKind.Weave: return { tag: "Weave" };
    case N.RemoteControlInterfaceKind.WeavePeer: return { tag: "WeavePeer" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlConnectionState(value: N.RemoteControlConnectionState): C.RemoteControlConnectionState {
  switch (value) {
    case N.RemoteControlConnectionState.Initializing: return { tag: "Initializing" };
    case N.RemoteControlConnectionState.Connected: return { tag: "Connected" };
    case N.RemoteControlConnectionState.Degraded: return { tag: "Degraded" };
    case N.RemoteControlConnectionState.Reconnecting: return { tag: "Reconnecting" };
    case N.RemoteControlConnectionState.Failed: return { tag: "Failed" };
    case N.RemoteControlConnectionState.Disconnected: return { tag: "Disconnected" };
    case N.RemoteControlConnectionState.Disabled: return { tag: "Disabled" };
    case N.RemoteControlConnectionState.Unknown: return { tag: "Unknown" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlInterfaceContinuation(value: N.RemoteControlInterfaceContinuation): C.RemoteControlInterfaceContinuation {
  switch (value.tag) {
    case N.RemoteControlInterfaceContinuation_Tags.Complete: return { tag: "Complete" };
    case N.RemoteControlInterfaceContinuation_Tags.More: return { tag: "More", data: { value: liftRemoteControlInterfaceCursor(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPowerOutcome(value: N.RemoteControlPowerOutcome): C.RemoteControlPowerOutcome {
  switch (value) {
    case N.RemoteControlPowerOutcome.Applied: return { tag: "Applied" };
    case N.RemoteControlPowerOutcome.UnknownInterface: return { tag: "UnknownInterface" };
    case N.RemoteControlPowerOutcome.Failed: return { tag: "Failed" };
    case N.RemoteControlPowerOutcome.Unchanged: return { tag: "Unchanged" };
    case N.RemoteControlPowerOutcome.Scheduled: return { tag: "Scheduled" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlModeOutcome(value: N.RemoteControlModeOutcome): C.RemoteControlModeOutcome {
  switch (value) {
    case N.RemoteControlModeOutcome.Applied: return { tag: "Applied" };
    case N.RemoteControlModeOutcome.UnknownInterface: return { tag: "UnknownInterface" };
    case N.RemoteControlModeOutcome.Failed: return { tag: "Failed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlGroupOutcome(value: N.RemoteControlGroupOutcome): C.RemoteControlGroupOutcome {
  switch (value) {
    case N.RemoteControlGroupOutcome.Applied: return { tag: "Applied" };
    case N.RemoteControlGroupOutcome.UnknownInterface: return { tag: "UnknownInterface" };
    case N.RemoteControlGroupOutcome.Failed: return { tag: "Failed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlDiscoveryGroupsInventoryOutcome(value: N.RemoteControlDiscoveryGroupsInventoryOutcome): C.RemoteControlDiscoveryGroupsInventoryOutcome {
  switch (value.tag) {
    case N.RemoteControlDiscoveryGroupsInventoryOutcome_Tags.Groups: return { tag: "Groups", data: { value: liftRemoteControlDiscoveryGroups(value.inner.value) } };
    case N.RemoteControlDiscoveryGroupsInventoryOutcome_Tags.UnknownInterface: return { tag: "UnknownInterface" };
    case N.RemoteControlDiscoveryGroupsInventoryOutcome_Tags.Unsupported: return { tag: "Unsupported" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlDiscoveryGroupsReplaceOutcome(value: N.RemoteControlDiscoveryGroupsReplaceOutcome): C.RemoteControlDiscoveryGroupsReplaceOutcome {
  switch (value) {
    case N.RemoteControlDiscoveryGroupsReplaceOutcome.Applied: return { tag: "Applied" };
    case N.RemoteControlDiscoveryGroupsReplaceOutcome.Unchanged: return { tag: "Unchanged" };
    case N.RemoteControlDiscoveryGroupsReplaceOutcome.UnknownInterface: return { tag: "UnknownInterface" };
    case N.RemoteControlDiscoveryGroupsReplaceOutcome.Unsupported: return { tag: "Unsupported" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlInterfacePeersOutcome(value: N.RemoteControlInterfacePeersOutcome): C.RemoteControlInterfacePeersOutcome {
  switch (value.tag) {
    case N.RemoteControlInterfacePeersOutcome_Tags.Page: return { tag: "Page", data: { value: liftRemoteControlInterfacePeerPage(value.inner.value) } };
    case N.RemoteControlInterfacePeersOutcome_Tags.UnknownInterface: return { tag: "UnknownInterface" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlInterfacePeerPage(value: N.RemoteControlInterfacePeerPage): C.RemoteControlInterfacePeerPage {
  return { id: liftRemoteControlInterfaceId(value.id), peers: value.peers.map(item => liftRemoteControlInterfacePeer(item)), continuation: liftRemoteControlPeerContinuation(value.continuation) };
}
export function liftRemoteControlInterfacePeer(value: N.RemoteControlInterfacePeer): C.RemoteControlInterfacePeer {
  return { id: liftRemoteControlInterfaceId(value.id), connection: liftRemoteControlConnectionState(value.connection), txBytes: value.txBytes, rxBytes: value.rxBytes, links: value.links, destinations: value.destinations, rateBytesPerSec: value.rateBytesPerSec, radio: liftRemoteControlRadioIndication(value.radio), details: liftRemoteControlPeerDetails(value.details) };
}
export function liftRemoteControlRadioIndication(value: N.RemoteControlRadioIndication): C.RemoteControlRadioIndication {
  switch (value.tag) {
    case N.RemoteControlRadioIndication_Tags.NotRadio: return { tag: "NotRadio" };
    case N.RemoteControlRadioIndication_Tags.Bluetooth: return { tag: "Bluetooth", data: { value: liftRemoteControlBluetoothIndication(value.inner.value) } };
    case N.RemoteControlRadioIndication_Tags.Wifi: return { tag: "Wifi", data: { value: liftRemoteControlWifiIndication(value.inner.value) } };
    case N.RemoteControlRadioIndication_Tags.LoRa: return { tag: "LoRa", data: { value: liftRemoteControlLoRaIndication(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlBluetoothIndication(value: N.RemoteControlBluetoothIndication): C.RemoteControlBluetoothIndication {
  switch (value.tag) {
    case N.RemoteControlBluetoothIndication_Tags.Pending: return { tag: "Pending" };
    case N.RemoteControlBluetoothIndication_Tags.Rssi: return { tag: "Rssi", data: { value: liftRemoteControlRssiDbm(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlRssiDbm(value: N.RemoteControlRssiDbm): C.RemoteControlRssiDbm {
  return { value: value.value };
}
export function liftRemoteControlWifiIndication(value: N.RemoteControlWifiIndication): C.RemoteControlWifiIndication {
  switch (value.tag) {
    case N.RemoteControlWifiIndication_Tags.Unavailable: return { tag: "Unavailable" };
    case N.RemoteControlWifiIndication_Tags.Pending: return { tag: "Pending" };
    case N.RemoteControlWifiIndication_Tags.Rssi: return { tag: "Rssi", data: { value: liftRemoteControlRssiDbm(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlLoRaIndication(value: N.RemoteControlLoRaIndication): C.RemoteControlLoRaIndication {
  switch (value.tag) {
    case N.RemoteControlLoRaIndication_Tags.Pending: return { tag: "Pending" };
    case N.RemoteControlLoRaIndication_Tags.Sample: return { tag: "Sample", data: { rssi: liftRemoteControlRssiDbm(value.inner.rssi), ...(value.inner.snr === undefined ? {} : { snr: liftRemoteControlSnrQuarterDb(value.inner.snr) }), ...(value.inner.quality === undefined ? {} : { quality: liftRemoteControlSignalQualityTenthsPercent(value.inner.quality) }) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlSnrQuarterDb(value: N.RemoteControlSnrQuarterDb): C.RemoteControlSnrQuarterDb {
  return { value: value.value };
}
export function liftRemoteControlSignalQualityTenthsPercent(value: N.RemoteControlSignalQualityTenthsPercent): C.RemoteControlSignalQualityTenthsPercent {
  return { value: value.value };
}
export function liftRemoteControlPeerDetails(value: N.RemoteControlPeerDetails): C.RemoteControlPeerDetails {
  switch (value.tag) {
    case N.RemoteControlPeerDetails_Tags.NotApplicable: return { tag: "NotApplicable" };
    case N.RemoteControlPeerDetails_Tags.Unknown: return { tag: "Unknown" };
    case N.RemoteControlPeerDetails_Tags.BleGatt: return { tag: "BleGatt" };
    case N.RemoteControlPeerDetails_Tags.BleCoc: return { tag: "BleCoc" };
    case N.RemoteControlPeerDetails_Tags.WifiRfChannel: return { tag: "WifiRfChannel", data: { value: value.inner.value } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPeerContinuation(value: N.RemoteControlPeerContinuation): C.RemoteControlPeerContinuation {
  switch (value.tag) {
    case N.RemoteControlPeerContinuation_Tags.Complete: return { tag: "Complete" };
    case N.RemoteControlPeerContinuation_Tags.More: return { tag: "More", data: { value: liftRemoteControlPeerCursor(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlInterfaceConfigOutcome(value: N.RemoteControlInterfaceConfigOutcome): C.RemoteControlInterfaceConfigOutcome {
  switch (value.tag) {
    case N.RemoteControlInterfaceConfigOutcome_Tags.Card: return { tag: "Card", data: { value: liftRemoteControlInterfaceCard(value.inner.value) } };
    case N.RemoteControlInterfaceConfigOutcome_Tags.UnknownInterface: return { tag: "UnknownInterface" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlInterfaceCard(value: N.RemoteControlInterfaceCard): C.RemoteControlInterfaceCard {
  return { name: value.name, group: value.group, config: value.config, failure: value.failure, destinations: value.destinations, transportedLinks: value.transportedLinks };
}
export function liftRemoteControlLoRaOutcome(value: N.RemoteControlLoRaOutcome): C.RemoteControlLoRaOutcome {
  switch (value) {
    case N.RemoteControlLoRaOutcome.Applied: return { tag: "Applied" };
    case N.RemoteControlLoRaOutcome.UnknownInterface: return { tag: "UnknownInterface" };
    case N.RemoteControlLoRaOutcome.Failed: return { tag: "Failed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlWifiStationOutcome(value: N.RemoteControlWifiStationOutcome): C.RemoteControlWifiStationOutcome {
  switch (value) {
    case N.RemoteControlWifiStationOutcome.Applied: return { tag: "Applied" };
    case N.RemoteControlWifiStationOutcome.UnknownInterface: return { tag: "UnknownInterface" };
    case N.RemoteControlWifiStationOutcome.Failed: return { tag: "Failed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlControllerInventory(value: N.RemoteControlControllerInventory): C.RemoteControlControllerInventory {
  return { hashes: value.hashes.map(item => liftRemoteControlIdentityHash(item)), continuation: liftRemoteControlControllerContinuation(value.continuation) };
}
export function liftRemoteControlControllerContinuation(value: N.RemoteControlControllerContinuation): C.RemoteControlControllerContinuation {
  switch (value.tag) {
    case N.RemoteControlControllerContinuation_Tags.Complete: return { tag: "Complete" };
    case N.RemoteControlControllerContinuation_Tags.More: return { tag: "More", data: { value: liftRemoteControlControllerCursor(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlAuthorizeControllerOutcome(value: N.RemoteControlAuthorizeControllerOutcome): C.RemoteControlAuthorizeControllerOutcome {
  switch (value) {
    case N.RemoteControlAuthorizeControllerOutcome.Applied: return { tag: "Applied" };
    case N.RemoteControlAuthorizeControllerOutcome.CapacityExhausted: return { tag: "CapacityExhausted" };
    case N.RemoteControlAuthorizeControllerOutcome.Failed: return { tag: "Failed" };
    case N.RemoteControlAuthorizeControllerOutcome.Forbidden: return { tag: "Forbidden" };
    case N.RemoteControlAuthorizeControllerOutcome.Busy: return { tag: "Busy" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlRevokeControllerOutcome(value: N.RemoteControlRevokeControllerOutcome): C.RemoteControlRevokeControllerOutcome {
  switch (value) {
    case N.RemoteControlRevokeControllerOutcome.Applied: return { tag: "Applied" };
    case N.RemoteControlRevokeControllerOutcome.NotFound: return { tag: "NotFound" };
    case N.RemoteControlRevokeControllerOutcome.Forbidden: return { tag: "Forbidden" };
    case N.RemoteControlRevokeControllerOutcome.Failed: return { tag: "Failed" };
    case N.RemoteControlRevokeControllerOutcome.Busy: return { tag: "Busy" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlBuildVersion(value: N.RemoteControlBuildVersion): C.RemoteControlBuildVersion {
  return { value: value.value };
}
export function liftRemoteControlPowerSnapshot(value: N.RemoteControlPowerSnapshot): C.RemoteControlPowerSnapshot {
  return { ...(value.battery === undefined ? {} : { battery: liftRemoteControlBatteryPercent(value.battery) }), externalPower: liftRemoteControlExternalPowerState(value.externalPower) };
}
export function liftRemoteControlBatteryPercent(value: N.RemoteControlBatteryPercent): C.RemoteControlBatteryPercent {
  return { value: value.value };
}
export function liftRemoteControlExternalPowerState(value: N.RemoteControlExternalPowerState): C.RemoteControlExternalPowerState {
  switch (value.tag) {
    case N.RemoteControlExternalPowerState_Tags.Absent: return { tag: "Absent" };
    case N.RemoteControlExternalPowerState_Tags.Present: return { tag: "Present", data: { charging: liftRemoteControlChargingState(value.inner.charging) } };
    case N.RemoteControlExternalPowerState_Tags.Unknown: return { tag: "Unknown" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlChargingState(value: N.RemoteControlChargingState): C.RemoteControlChargingState {
  switch (value) {
    case N.RemoteControlChargingState.Charging: return { tag: "Charging" };
    case N.RemoteControlChargingState.Idle: return { tag: "Idle" };
    case N.RemoteControlChargingState.Unknown: return { tag: "Unknown" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlSleepOutcome(value: N.RemoteControlSleepOutcome): C.RemoteControlSleepOutcome {
  switch (value) {
    case N.RemoteControlSleepOutcome.Applied: return { tag: "Applied" };
    case N.RemoteControlSleepOutcome.Unavailable: return { tag: "Unavailable" };
    case N.RemoteControlSleepOutcome.Failed: return { tag: "Failed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlApplyOutcome(value: N.RemoteControlApplyOutcome): C.RemoteControlApplyOutcome {
  switch (value) {
    case N.RemoteControlApplyOutcome.Applied: return { tag: "Applied" };
    case N.RemoteControlApplyOutcome.Unchanged: return { tag: "Unchanged" };
    case N.RemoteControlApplyOutcome.Scheduled: return { tag: "Scheduled" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlWifiStageOutcome(value: N.RemoteControlWifiStageOutcome): C.RemoteControlWifiStageOutcome {
  switch (value.tag) {
    case N.RemoteControlWifiStageOutcome_Tags.Staged: return { tag: "Staged", data: { value: liftRemoteControlWifiCredentialRevision(value.inner.value) } };
    case N.RemoteControlWifiStageOutcome_Tags.InvalidCredentials: return { tag: "InvalidCredentials" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlWifiTransactionStatus(value: N.RemoteControlWifiTransactionStatus): C.RemoteControlWifiTransactionStatus {
  switch (value.tag) {
    case N.RemoteControlWifiTransactionStatus_Tags.FactoryProvisioning: return { tag: "FactoryProvisioning" };
    case N.RemoteControlWifiTransactionStatus_Tags.Confirmed: return { tag: "Confirmed", data: { revision: liftRemoteControlWifiCredentialRevision(value.inner.revision) } };
    case N.RemoteControlWifiTransactionStatus_Tags.Staged: return { tag: "Staged", data: { revision: liftRemoteControlWifiCredentialRevision(value.inner.revision) } };
    case N.RemoteControlWifiTransactionStatus_Tags.AwaitingConfirmation: return { tag: "AwaitingConfirmation", data: { revision: liftRemoteControlWifiCredentialRevision(value.inner.revision), remaining: liftRemoteControlWifiConfirmationRemaining(value.inner.remaining) } };
    case N.RemoteControlWifiTransactionStatus_Tags.RollingBack: return { tag: "RollingBack", data: { rejectedRevision: liftRemoteControlWifiCredentialRevision(value.inner.rejectedRevision) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlWifiConfirmationRemaining(value: N.RemoteControlWifiConfirmationRemaining): C.RemoteControlWifiConfirmationRemaining {
  return { value: value.value };
}
export function liftRemoteControlProtocolError(value: N.RemoteControlProtocolError): C.RemoteControlProtocolError {
  switch (value.tag) {
    case N.RemoteControlProtocolError_Tags.MalformedRequest: return { tag: "MalformedRequest" };
    case N.RemoteControlProtocolError_Tags.UnsupportedVersion: return { tag: "UnsupportedVersion", data: { found: value.inner.found } };
    case N.RemoteControlProtocolError_Tags.UnknownRequestKind: return { tag: "UnknownRequestKind", data: { found: value.inner.found } };
    case N.RemoteControlProtocolError_Tags.UnsupportedRequest: return { tag: "UnsupportedRequest", data: { request: liftRemoteControlRequestKind(value.inner.request) } };
    case N.RemoteControlProtocolError_Tags.Busy: return { tag: "Busy", data: { request: liftRemoteControlRequestKind(value.inner.request) } };
    case N.RemoteControlProtocolError_Tags.ApplyFailed: return { tag: "ApplyFailed", data: { request: liftRemoteControlRequestKind(value.inner.request) } };
    case N.RemoteControlProtocolError_Tags.PersistenceFailed: return { tag: "PersistenceFailed", data: { request: liftRemoteControlRequestKind(value.inner.request) } };
    case N.RemoteControlProtocolError_Tags.RollbackFailed: return { tag: "RollbackFailed", data: { request: liftRemoteControlRequestKind(value.inner.request) } };
    case N.RemoteControlProtocolError_Tags.InternalFailure: return { tag: "InternalFailure", data: { request: liftRemoteControlRequestKind(value.inner.request) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlExchangeSettlement(value: N.RemoteControlExchangeSettlement): C.RemoteControlExchangeSettlement {
  switch (value.tag) {
    case N.RemoteControlExchangeSettlement_Tags.Completed: return { tag: "Completed", data: { response: liftRemoteControlResponse(value.inner.response), rttMillis: value.inner.rttMillis } };
    case N.RemoteControlExchangeSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlNativeRemoteControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlNativeRemoteControlError(value: N.RemoteControlNativeRemoteControlError): C.RemoteControlNativeRemoteControlError {
  switch (value.tag) {
    case N.RemoteControlNativeRemoteControlError_Tags.Admission: return { tag: "Admission", data: { value: liftRemoteControlNativeSubmitError(value.inner.value) } };
    case N.RemoteControlNativeRemoteControlError_Tags.Connect: return { tag: "Connect", data: { value: liftRemoteControlConnectRemoteControlTargetError(value.inner.value) } };
    case N.RemoteControlNativeRemoteControlError_Tags.Exchange: return { tag: "Exchange", data: { value: liftRemoteControlError(value.inner.value) } };
    case N.RemoteControlNativeRemoteControlError_Tags.TargetOperation: return { tag: "TargetOperation", data: { value: liftRemoteControlTargetOperationError(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlNativeSubmitError(value: N.RemoteControlNativeSubmitError): C.RemoteControlNativeSubmitError {
  switch (value) {
    case N.RemoteControlNativeSubmitError.Busy: return { tag: "Busy" };
    case N.RemoteControlNativeSubmitError.Stopped: return { tag: "Stopped" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlConnectRemoteControlTargetError(value: N.RemoteControlConnectRemoteControlTargetError): C.RemoteControlConnectRemoteControlTargetError {
  switch (value.tag) {
    case N.RemoteControlConnectRemoteControlTargetError_Tags.Resolve: return { tag: "Resolve", data: { value: liftRemoteControlResolveRemoteControlTargetControlError(value.inner.value) } };
    case N.RemoteControlConnectRemoteControlTargetError_Tags.EstablishLink: return { tag: "EstablishLink", data: { value: liftRemoteControlSendErrorRemoteControlEstablishLinkFailure(value.inner.value) } };
    case N.RemoteControlConnectRemoteControlTargetError_Tags.Identify: return { tag: "Identify", data: { value: liftRemoteControlSendErrorRemoteControlIdentifyFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlResolveRemoteControlTargetControlError(value: N.RemoteControlResolveRemoteControlTargetControlError): C.RemoteControlResolveRemoteControlTargetControlError {
  switch (value) {
    case N.RemoteControlResolveRemoteControlTargetControlError.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlResolveRemoteControlTargetControlError.Busy: return { tag: "Busy" };
    case N.RemoteControlResolveRemoteControlTargetControlError.Unavailable: return { tag: "Unavailable" };
    case N.RemoteControlResolveRemoteControlTargetControlError.TargetNotAuthorized: return { tag: "TargetNotAuthorized" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlSendErrorRemoteControlEstablishLinkFailure(value: N.RemoteControlSendErrorRemoteControlEstablishLinkFailure): C.RemoteControlSendErrorRemoteControlEstablishLinkFailure {
  switch (value.tag) {
    case N.RemoteControlSendErrorRemoteControlEstablishLinkFailure_Tags.PayloadTooLarge: return { tag: "PayloadTooLarge" };
    case N.RemoteControlSendErrorRemoteControlEstablishLinkFailure_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlSendErrorRemoteControlEstablishLinkFailure_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlSendErrorRemoteControlEstablishLinkFailure_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlEstablishLinkFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlEstablishLinkFailure(value: N.RemoteControlEstablishLinkFailure): C.RemoteControlEstablishLinkFailure {
  switch (value.tag) {
    case N.RemoteControlEstablishLinkFailure_Tags.Rejected: return { tag: "Rejected", data: { value: liftRemoteControlEstablishLinkRejection(value.inner.value) } };
    case N.RemoteControlEstablishLinkFailure_Tags.WriteFailed: return { tag: "WriteFailed", data: { value: liftRemoteControlWriteEstablishLinkRejection(value.inner.value) } };
    case N.RemoteControlEstablishLinkFailure_Tags.Timeout: return { tag: "Timeout" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlEstablishLinkRejection(value: N.RemoteControlEstablishLinkRejection): C.RemoteControlEstablishLinkRejection {
  switch (value) {
    case N.RemoteControlEstablishLinkRejection.NoRouteToDestination: return { tag: "NoRouteToDestination" };
    case N.RemoteControlEstablishLinkRejection.NotDirectlyReachable: return { tag: "NotDirectlyReachable" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlWriteEstablishLinkRejection(value: N.RemoteControlWriteEstablishLinkRejection): C.RemoteControlWriteEstablishLinkRejection {
  switch (value) {
    case N.RemoteControlWriteEstablishLinkRejection.RouteVanished: return { tag: "RouteVanished" };
    case N.RemoteControlWriteEstablishLinkRejection.Serialize: return { tag: "Serialize" };
    case N.RemoteControlWriteEstablishLinkRejection.LinkTableFull: return { tag: "LinkTableFull" };
    case N.RemoteControlWriteEstablishLinkRejection.DuplicateLinkId: return { tag: "DuplicateLinkId" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlSendErrorRemoteControlIdentifyFailure(value: N.RemoteControlSendErrorRemoteControlIdentifyFailure): C.RemoteControlSendErrorRemoteControlIdentifyFailure {
  switch (value.tag) {
    case N.RemoteControlSendErrorRemoteControlIdentifyFailure_Tags.PayloadTooLarge: return { tag: "PayloadTooLarge" };
    case N.RemoteControlSendErrorRemoteControlIdentifyFailure_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlSendErrorRemoteControlIdentifyFailure_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlSendErrorRemoteControlIdentifyFailure_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlIdentifyFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlIdentifyFailure(value: N.RemoteControlIdentifyFailure): C.RemoteControlIdentifyFailure {
  switch (value.tag) {
    case N.RemoteControlIdentifyFailure_Tags.Rejected: return { tag: "Rejected", data: { value: liftRemoteControlIdentifyRejection(value.inner.value) } };
    case N.RemoteControlIdentifyFailure_Tags.WriteFailed: return { tag: "WriteFailed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlIdentifyRejection(value: N.RemoteControlIdentifyRejection): C.RemoteControlIdentifyRejection {
  switch (value) {
    case N.RemoteControlIdentifyRejection.NoSuchLink: return { tag: "NoSuchLink" };
    case N.RemoteControlIdentifyRejection.LinkNotActive: return { tag: "LinkNotActive" };
    case N.RemoteControlIdentifyRejection.NotInitiator: return { tag: "NotInitiator" };
    case N.RemoteControlIdentifyRejection.IdentityNotHeld: return { tag: "IdentityNotHeld" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlError(value: N.RemoteControlError): C.RemoteControlError {
  switch (value.tag) {
    case N.RemoteControlError_Tags.UnsupportedRequestKind: return { tag: "UnsupportedRequestKind", data: { value: liftRemoteControlRequestKind(value.inner.value) } };
    case N.RemoteControlError_Tags.Encode: return { tag: "Encode", data: { value: liftRemoteControlMessageWriteError(value.inner.value) } };
    case N.RemoteControlError_Tags.Request: return { tag: "Request", data: { value: liftRemoteControlSendErrorRemoteControlSendRequestFailure(value.inner.value) } };
    case N.RemoteControlError_Tags.Response: return { tag: "Response", data: { value: liftRemoteControlResponseParseError(value.inner.value) } };
    case N.RemoteControlError_Tags.Remote: return { tag: "Remote", data: { value: liftRemoteControlProtocolError(value.inner.value) } };
    case N.RemoteControlError_Tags.UnexpectedResponse: return { tag: "UnexpectedResponse", data: { expected: liftRemoteControlResponseKind(value.inner.expected), found: liftRemoteControlResponseKind(value.inner.found) } };
    case N.RemoteControlError_Tags.AnnounceSelf: return { tag: "AnnounceSelf", data: { value: liftRemoteControlAnnounceSelfFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlMessageWriteError(value: N.RemoteControlMessageWriteError): C.RemoteControlMessageWriteError {
  switch (value) {
    case N.RemoteControlMessageWriteError.BufferTooShort: return { tag: "BufferTooShort" };
    case N.RemoteControlMessageWriteError.InvalidRequestSet: return { tag: "InvalidRequestSet" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlSendErrorRemoteControlSendRequestFailure(value: N.RemoteControlSendErrorRemoteControlSendRequestFailure): C.RemoteControlSendErrorRemoteControlSendRequestFailure {
  switch (value.tag) {
    case N.RemoteControlSendErrorRemoteControlSendRequestFailure_Tags.PayloadTooLarge: return { tag: "PayloadTooLarge" };
    case N.RemoteControlSendErrorRemoteControlSendRequestFailure_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlSendErrorRemoteControlSendRequestFailure_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlSendErrorRemoteControlSendRequestFailure_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlSendRequestFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlSendRequestFailure(value: N.RemoteControlSendRequestFailure): C.RemoteControlSendRequestFailure {
  switch (value.tag) {
    case N.RemoteControlSendRequestFailure_Tags.Rejected: return { tag: "Rejected", data: { value: liftRemoteControlSendRequestRejection(value.inner.value) } };
    case N.RemoteControlSendRequestFailure_Tags.WriteFailed: return { tag: "WriteFailed" };
    case N.RemoteControlSendRequestFailure_Tags.Culled: return { tag: "Culled" };
    case N.RemoteControlSendRequestFailure_Tags.Timeout: return { tag: "Timeout" };
    case N.RemoteControlSendRequestFailure_Tags.LinkClosed: return { tag: "LinkClosed" };
    case N.RemoteControlSendRequestFailure_Tags.ResponseTooLarge: return { tag: "ResponseTooLarge" };
    case N.RemoteControlSendRequestFailure_Tags.ResponseTransferFailed: return { tag: "ResponseTransferFailed", data: { value: liftRemoteControlResourceFailureCause(value.inner.value) } };
    case N.RemoteControlSendRequestFailure_Tags.ResourceCapacity: return { tag: "ResourceCapacity" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlSendRequestRejection(value: N.RemoteControlSendRequestRejection): C.RemoteControlSendRequestRejection {
  switch (value) {
    case N.RemoteControlSendRequestRejection.NoSuchLink: return { tag: "NoSuchLink" };
    case N.RemoteControlSendRequestRejection.LinkNotActive: return { tag: "LinkNotActive" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlResourceFailureCause(value: N.RemoteControlResourceFailureCause): C.RemoteControlResourceFailureCause {
  switch (value.tag) {
    case N.RemoteControlResourceFailureCause_Tags.CancelledBySender: return { tag: "CancelledBySender" };
    case N.RemoteControlResourceFailureCause_Tags.RefusedHashmapUpdate: return { tag: "RefusedHashmapUpdate", data: { value: liftRemoteControlApplyHashmapUpdateError(value.inner.value) } };
    case N.RemoteControlResourceFailureCause_Tags.RetriesExhausted: return { tag: "RetriesExhausted" };
    case N.RemoteControlResourceFailureCause_Tags.LinkVanished: return { tag: "LinkVanished" };
    case N.RemoteControlResourceFailureCause_Tags.TransferUnopenable: return { tag: "TransferUnopenable" };
    case N.RemoteControlResourceFailureCause_Tags.TransferCorrupt: return { tag: "TransferCorrupt" };
    case N.RemoteControlResourceFailureCause_Tags.ProofUnsendable: return { tag: "ProofUnsendable" };
    case N.RemoteControlResourceFailureCause_Tags.DecompressionFailed: return { tag: "DecompressionFailed" };
    case N.RemoteControlResourceFailureCause_Tags.DecompressionTimedOut: return { tag: "DecompressionTimedOut" };
    case N.RemoteControlResourceFailureCause_Tags.OpenTimedOut: return { tag: "OpenTimedOut" };
    case N.RemoteControlResourceFailureCause_Tags.MetadataOverrun: return { tag: "MetadataOverrun" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlApplyHashmapUpdateError(value: N.RemoteControlApplyHashmapUpdateError): C.RemoteControlApplyHashmapUpdateError {
  switch (value) {
    case N.RemoteControlApplyHashmapUpdateError.BeyondPartCount: return { tag: "BeyondPartCount" };
    case N.RemoteControlApplyHashmapUpdateError.SkipsAhead: return { tag: "SkipsAhead" };
    case N.RemoteControlApplyHashmapUpdateError.HashmapTooLong: return { tag: "HashmapTooLong" };
    case N.RemoteControlApplyHashmapUpdateError.HashmapRagged: return { tag: "HashmapRagged" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlResponseParseError(value: N.RemoteControlResponseParseError): C.RemoteControlResponseParseError {
  switch (value.tag) {
    case N.RemoteControlResponseParseError_Tags.Truncated: return { tag: "Truncated" };
    case N.RemoteControlResponseParseError_Tags.UnsupportedVersion: return { tag: "UnsupportedVersion", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownResponseKind: return { tag: "UnknownResponseKind", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownAnnounceSelfOutcome: return { tag: "UnknownAnnounceSelfOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownPowerOutcome: return { tag: "UnknownPowerOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownModeOutcome: return { tag: "UnknownModeOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownGroupOutcome: return { tag: "UnknownGroupOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownLoRaOutcome: return { tag: "UnknownLoRaOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownWifiStationOutcome: return { tag: "UnknownWifiStationOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownAuthorizeControllerOutcome: return { tag: "UnknownAuthorizeControllerOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownRevokeControllerOutcome: return { tag: "UnknownRevokeControllerOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownSleepOutcome: return { tag: "UnknownSleepOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownApplyOutcome: return { tag: "UnknownApplyOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownWifiStageOutcome: return { tag: "UnknownWifiStageOutcome", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownWifiTransactionStatus: return { tag: "UnknownWifiTransactionStatus", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownProtocolErrorKind: return { tag: "UnknownProtocolErrorKind", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownRequestKind: return { tag: "UnknownRequestKind", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownInterfaceKind: return { tag: "UnknownInterfaceKind", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownInterfaceMode: return { tag: "UnknownInterfaceMode", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.UnknownConnectionState: return { tag: "UnknownConnectionState", data: { found: value.inner.found } };
    case N.RemoteControlResponseParseError_Tags.NonCanonicalRequestSet: return { tag: "NonCanonicalRequestSet" };
    case N.RemoteControlResponseParseError_Tags.NonCanonicalCursor: return { tag: "NonCanonicalCursor" };
    case N.RemoteControlResponseParseError_Tags.Malformed: return { tag: "Malformed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlResponseKind(value: N.RemoteControlResponseKind): C.RemoteControlResponseKind {
  switch (value) {
    case N.RemoteControlResponseKind.Describe: return { tag: "Describe" };
    case N.RemoteControlResponseKind.AnnounceSelf: return { tag: "AnnounceSelf" };
    case N.RemoteControlResponseKind.InventoryInterfaces: return { tag: "InventoryInterfaces" };
    case N.RemoteControlResponseKind.SetInterfacePower: return { tag: "SetInterfacePower" };
    case N.RemoteControlResponseKind.SleepRadios: return { tag: "SleepRadios" };
    case N.RemoteControlResponseKind.WakeRadios: return { tag: "WakeRadios" };
    case N.RemoteControlResponseKind.SetInterfaceMode: return { tag: "SetInterfaceMode" };
    case N.RemoteControlResponseKind.SetInterfaceGroup: return { tag: "SetInterfaceGroup" };
    case N.RemoteControlResponseKind.InventoryInterfacePeers: return { tag: "InventoryInterfacePeers" };
    case N.RemoteControlResponseKind.InventoryInterfaceConfig: return { tag: "InventoryInterfaceConfig" };
    case N.RemoteControlResponseKind.SetInterfaceLoRaProfile: return { tag: "SetInterfaceLoRaProfile" };
    case N.RemoteControlResponseKind.DescribeBuild: return { tag: "DescribeBuild" };
    case N.RemoteControlResponseKind.SetInterfaceWifiStation: return { tag: "SetInterfaceWifiStation" };
    case N.RemoteControlResponseKind.InventoryControllers: return { tag: "InventoryControllers" };
    case N.RemoteControlResponseKind.AuthorizeController: return { tag: "AuthorizeController" };
    case N.RemoteControlResponseKind.RevokeController: return { tag: "RevokeController" };
    case N.RemoteControlResponseKind.DescribePower: return { tag: "DescribePower" };
    case N.RemoteControlResponseKind.SetSystemPower: return { tag: "SetSystemPower" };
    case N.RemoteControlResponseKind.SetGnssPower: return { tag: "SetGnssPower" };
    case N.RemoteControlResponseKind.SetDisplayVisibility: return { tag: "SetDisplayVisibility" };
    case N.RemoteControlResponseKind.SetDisplayAutoOff: return { tag: "SetDisplayAutoOff" };
    case N.RemoteControlResponseKind.SetStationUplink: return { tag: "SetStationUplink" };
    case N.RemoteControlResponseKind.SetEspRadioMode: return { tag: "SetEspRadioMode" };
    case N.RemoteControlResponseKind.StageWifiCredentials: return { tag: "StageWifiCredentials" };
    case N.RemoteControlResponseKind.ActivateWifiCredentials: return { tag: "ActivateWifiCredentials" };
    case N.RemoteControlResponseKind.ConfirmWifiCredentials: return { tag: "ConfirmWifiCredentials" };
    case N.RemoteControlResponseKind.CancelWifiCredentials: return { tag: "CancelWifiCredentials" };
    case N.RemoteControlResponseKind.InspectWifiTransaction: return { tag: "InspectWifiTransaction" };
    case N.RemoteControlResponseKind.InventoryInterfaceDiscoveryGroups: return { tag: "InventoryInterfaceDiscoveryGroups" };
    case N.RemoteControlResponseKind.ReplaceInterfaceDiscoveryGroups: return { tag: "ReplaceInterfaceDiscoveryGroups" };
    case N.RemoteControlResponseKind.ProtocolError: return { tag: "ProtocolError" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlAnnounceSelfFailure(value: N.RemoteControlAnnounceSelfFailure): C.RemoteControlAnnounceSelfFailure {
  switch (value) {
    case N.RemoteControlAnnounceSelfFailure.Unavailable: return { tag: "Unavailable" };
    case N.RemoteControlAnnounceSelfFailure.Rejected: return { tag: "Rejected" };
    case N.RemoteControlAnnounceSelfFailure.WriteFailed: return { tag: "WriteFailed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlTargetOperationError(value: N.RemoteControlTargetOperationError): C.RemoteControlTargetOperationError {
  switch (value.tag) {
    case N.RemoteControlTargetOperationError_Tags.NotPermitted: return { tag: "NotPermitted", data: { value: liftRemoteControlRequestKind(value.inner.value) } };
    case N.RemoteControlTargetOperationError_Tags.Exchange: return { tag: "Exchange", data: { value: liftRemoteControlError(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function lowerRemoteControlNativeRemoteControlConfig(value: C.RemoteControlNativeRemoteControlConfig): N.RemoteControlNativeRemoteControlConfig {
  return { controllerIdentity: lowerIdentityConfig(value.controllerIdentity), targetIdentity: lowerIdentityConfig(value.targetIdentity), initialControllerGrants: value.initialControllerGrants.map(item => lowerRemoteControlControllerGrant(item)), selfAnnouncement: lowerRemoteControlSelfAnnouncement(value.selfAnnouncement), capabilities: lowerRemoteControlCapabilities(value.capabilities) };
}
export function lowerRemoteControlControllerGrant(value: C.RemoteControlControllerGrant): N.RemoteControlControllerGrant {
  return { controller: lowerRemoteControlControllerIdentity(value.controller), authority: lowerRemoteControlControllerAuthority(value.authority), permittedRequests: lowerRemoteControlRequestSet(value.permittedRequests) };
}
export function liftRemoteControlControllerGrant(value: N.RemoteControlControllerGrant): C.RemoteControlControllerGrant {
  return { controller: liftRemoteControlControllerIdentity(value.controller), authority: liftRemoteControlControllerAuthority(value.authority), permittedRequests: liftRemoteControlRequestSet(value.permittedRequests) };
}
export function lowerRemoteControlControllerAuthority(value: C.RemoteControlControllerAuthority): N.RemoteControlControllerAuthority {
  switch (value.tag) {
    case "Operator": return N.RemoteControlControllerAuthority.Operator;
    case "Administrator": return N.RemoteControlControllerAuthority.Administrator;
    default: return unexpected(value);
  }
}
export function liftRemoteControlControllerAuthority(value: N.RemoteControlControllerAuthority): C.RemoteControlControllerAuthority {
  switch (value) {
    case N.RemoteControlControllerAuthority.Operator: return { tag: "Operator" };
    case N.RemoteControlControllerAuthority.Administrator: return { tag: "Administrator" };
    default: return unexpected(value);
  }
}
export function lowerRemoteControlSelfAnnouncement(value: C.RemoteControlSelfAnnouncement): N.RemoteControlSelfAnnouncement {
  switch (value.tag) {
    case "Unavailable": return N.RemoteControlSelfAnnouncement.Unavailable.new();
    case "Destination": return N.RemoteControlSelfAnnouncement.Destination.new({ value: lowerRemoteControlDestinationHash(value.data.value) });
    default: return unexpected(value);
  }
}
export function lowerRemoteControlDestinationHash(value: C.RemoteControlDestinationHash): N.RemoteControlDestinationHash {
  return { value: value.value };
}
export function liftRemoteControlDestinationHash(value: N.RemoteControlDestinationHash): C.RemoteControlDestinationHash {
  return { value: value.value };
}
export function lowerRemoteControlCapabilities(value: C.RemoteControlCapabilities): N.RemoteControlCapabilities {
  return { requests: lowerRemoteControlRequestSet(value.requests) };
}
export function liftRemoteControlNativeRemoteControlEvent(value: N.RemoteControlNativeRemoteControlEvent): C.RemoteControlNativeRemoteControlEvent {
  switch (value.tag) {
    case N.RemoteControlNativeRemoteControlEvent_Tags.PairingAvailable: return { tag: "PairingAvailable", data: { endpoint: liftRemoteControlPairingEndpoint(value.inner.endpoint), observedAt: liftRemoteControlInstantMillis(value.inner.observedAt), expiresAt: liftRemoteControlInstantMillis(value.inner.expiresAt), hops: value.inner.hops, sourceInterface: liftRemoteControlInterfaceId(value.inner.sourceInterface), publicAppData: value.inner.publicAppData } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.TargetConfirmationRequired: return { tag: "TargetConfirmationRequired", data: { confirmation: liftRemoteControlNativeRemoteControlConfirmation(value.inner.confirmation), window: liftRemoteControlTargetPairingAttemptWindow(value.inner.window) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.TargetControllerCommitted: return { tag: "TargetControllerCommitted", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.TargetAuthorizationRequired: return { tag: "TargetAuthorizationRequired", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), grant: liftRemoteControlControllerGrant(value.inner.grant) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.TargetAuthorizationPersisted: return { tag: "TargetAuthorizationPersisted", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.TargetExpiredDuringAuthorization: return { tag: "TargetExpiredDuringAuthorization", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.ControllerConfirmationRequired: return { tag: "ControllerConfirmationRequired", data: { confirmation: liftRemoteControlNativeRemoteControlConfirmation(value.inner.confirmation), window: liftRemoteControlControllerPairingAttemptWindow(value.inner.window) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.ControllerPersistenceRequired: return { tag: "ControllerPersistenceRequired", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), context: liftRemoteControlPairingContext(value.inner.context), target: liftRemoteControlTargetIdentity(value.inner.target), authority: liftRemoteControlControllerAuthority(value.inner.authority), permittedRequests: liftRemoteControlRequestSet(value.inner.permittedRequests) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.ControllerAuthorizationPersisted: return { tag: "ControllerAuthorizationPersisted", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.ControllerAuthorizationPersistenceFailed: return { tag: "ControllerAuthorizationPersistenceFailed", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.ControllerExpired: return { tag: "ControllerExpired", data: { aborted: liftRemoteControlControllerPairingAborted(value.inner.aborted) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.ControllerLinkClosed: return { tag: "ControllerLinkClosed", data: { aborted: liftRemoteControlControllerPairingAborted(value.inner.aborted) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.TargetExpired: return { tag: "TargetExpired", data: { aborted: liftRemoteControlTargetPairingAborted(value.inner.aborted) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.TargetLinkClosed: return { tag: "TargetLinkClosed", data: { aborted: liftRemoteControlTargetPairingAborted(value.inner.aborted) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.TargetCompletionRetentionExpired: return { tag: "TargetCompletionRetentionExpired", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlNativeRemoteControlEvent_Tags.TargetCompletionLinkClosed: return { tag: "TargetCompletionLinkClosed", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingEndpoint(value: N.RemoteControlPairingEndpoint): C.RemoteControlPairingEndpoint {
  return { destinationHash: liftRemoteControlDestinationHash(value.destinationHash) };
}
export function lowerRemoteControlPairingEndpoint(value: C.RemoteControlPairingEndpoint): N.RemoteControlPairingEndpoint {
  return { destinationHash: lowerRemoteControlDestinationHash(value.destinationHash) };
}
export function liftRemoteControlInstantMillis(value: N.RemoteControlInstantMillis): C.RemoteControlInstantMillis {
  return { value: value.value };
}
export function lowerRemoteControlInstantMillis(value: C.RemoteControlInstantMillis): N.RemoteControlInstantMillis {
  return { value: value.value };
}
export function liftRemoteControlNativeRemoteControlConfirmation(value: N.RemoteControlNativeRemoteControlConfirmation): C.RemoteControlNativeRemoteControlConfirmation {
  return { attemptId: liftRemoteControlPairingAttemptId(value.attemptId), context: liftRemoteControlPairingContext(value.context), controller: liftRemoteControlControllerIdentity(value.controller), target: liftRemoteControlTargetIdentity(value.target), permissions: liftRemoteControlPairingPermissions(value.permissions), confirmationCode: value.confirmationCode };
}
export function liftRemoteControlPairingAttemptId(value: N.RemoteControlPairingAttemptId): C.RemoteControlPairingAttemptId {
  return { value: value.value };
}
export function lowerRemoteControlPairingAttemptId(value: C.RemoteControlPairingAttemptId): N.RemoteControlPairingAttemptId {
  return { value: value.value };
}
export function liftRemoteControlPairingContext(value: N.RemoteControlPairingContext): C.RemoteControlPairingContext {
  return { endpoint: liftRemoteControlPairingEndpoint(value.endpoint), linkId: liftRemoteControlLinkId(value.linkId) };
}
export function lowerRemoteControlPairingContext(value: C.RemoteControlPairingContext): N.RemoteControlPairingContext {
  return { endpoint: lowerRemoteControlPairingEndpoint(value.endpoint), linkId: lowerRemoteControlLinkId(value.linkId) };
}
export function liftRemoteControlLinkId(value: N.RemoteControlLinkId): C.RemoteControlLinkId {
  return { value: value.value };
}
export function lowerRemoteControlLinkId(value: C.RemoteControlLinkId): N.RemoteControlLinkId {
  return { value: value.value };
}
export function liftRemoteControlTargetIdentity(value: N.RemoteControlTargetIdentity): C.RemoteControlTargetIdentity {
  return { publicKeys: value.publicKeys };
}
export function lowerRemoteControlTargetIdentity(value: C.RemoteControlTargetIdentity): N.RemoteControlTargetIdentity {
  return { publicKeys: value.publicKeys };
}
export function liftRemoteControlPairingPermissions(value: N.RemoteControlPairingPermissions): C.RemoteControlPairingPermissions {
  return { authority: liftRemoteControlControllerAuthority(value.authority), permittedRequests: liftRemoteControlRequestSet(value.permittedRequests) };
}
export function lowerRemoteControlPairingPermissions(value: C.RemoteControlPairingPermissions): N.RemoteControlPairingPermissions {
  return { authority: lowerRemoteControlControllerAuthority(value.authority), permittedRequests: lowerRemoteControlRequestSet(value.permittedRequests) };
}
export function liftRemoteControlTargetPairingAttemptWindow(value: N.RemoteControlTargetPairingAttemptWindow): C.RemoteControlTargetPairingAttemptWindow {
  return { startedAt: liftRemoteControlInstantMillis(value.startedAt), attemptTimeout: liftRemoteControlPairingAttemptTimeout(value.attemptTimeout), expiresAt: liftRemoteControlInstantMillis(value.expiresAt) };
}
export function liftRemoteControlPairingAttemptTimeout(value: N.RemoteControlPairingAttemptTimeout): C.RemoteControlPairingAttemptTimeout {
  return { value: value.value };
}
export function lowerRemoteControlPairingAttemptTimeout(value: C.RemoteControlPairingAttemptTimeout): N.RemoteControlPairingAttemptTimeout {
  return { value: value.value };
}
export function liftRemoteControlControllerPairingAttemptWindow(value: N.RemoteControlControllerPairingAttemptWindow): C.RemoteControlControllerPairingAttemptWindow {
  return { offeredAt: liftRemoteControlInstantMillis(value.offeredAt), attemptTimeout: liftRemoteControlPairingAttemptTimeout(value.attemptTimeout), expiresAt: liftRemoteControlInstantMillis(value.expiresAt) };
}
export function liftRemoteControlControllerPairingAborted(value: N.RemoteControlControllerPairingAborted): C.RemoteControlControllerPairingAborted {
  switch (value.tag) {
    case N.RemoteControlControllerPairingAborted_Tags.AwaitingOffer: return { tag: "AwaitingOffer", data: { context: liftRemoteControlPairingContext(value.inner.context) } };
    case N.RemoteControlControllerPairingAborted_Tags.AwaitingApproval: return { tag: "AwaitingApproval", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), context: liftRemoteControlPairingContext(value.inner.context) } };
    case N.RemoteControlControllerPairingAborted_Tags.AwaitingCompletion: return { tag: "AwaitingCompletion", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), context: liftRemoteControlPairingContext(value.inner.context) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlTargetPairingAborted(value: N.RemoteControlTargetPairingAborted): C.RemoteControlTargetPairingAborted {
  switch (value.tag) {
    case N.RemoteControlTargetPairingAborted_Tags.OfferPrepared: return { tag: "OfferPrepared", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), context: liftRemoteControlPairingContext(value.inner.context) } };
    case N.RemoteControlTargetPairingAborted_Tags.AwaitingBoth: return { tag: "AwaitingBoth", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), context: liftRemoteControlPairingContext(value.inner.context) } };
    case N.RemoteControlTargetPairingAborted_Tags.AwaitingTargetApproval: return { tag: "AwaitingTargetApproval", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), context: liftRemoteControlPairingContext(value.inner.context), responder: liftRemoteControlTargetPairingResponder(value.inner.responder) } };
    case N.RemoteControlTargetPairingAborted_Tags.AwaitingControllerCommit: return { tag: "AwaitingControllerCommit", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), context: liftRemoteControlPairingContext(value.inner.context) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlTargetPairingResponder(value: N.RemoteControlTargetPairingResponder): C.RemoteControlTargetPairingResponder {
  return { linkId: liftRemoteControlLinkId(value.linkId), requestId: liftRemoteControlRequestId(value.requestId) };
}
export function liftRemoteControlRequestId(value: N.RemoteControlRequestId): C.RemoteControlRequestId {
  return { value: value.value };
}
export function lowerRemoteControlBeginRemoteControlControllerPairing(value: C.RemoteControlBeginRemoteControlControllerPairing): N.RemoteControlBeginRemoteControlControllerPairing {
  return { context: lowerRemoteControlPairingContext(value.context), invitationCode: lowerRemoteControlPairingInvitationCode(value.invitationCode), pairingExpiresAt: lowerRemoteControlInstantMillis(value.pairingExpiresAt) };
}
export function lowerRemoteControlPairingInvitationCode(value: C.RemoteControlPairingInvitationCode): N.RemoteControlPairingInvitationCode {
  return { value: value.value };
}
export function liftRemoteControlPairingInvitationCode(value: N.RemoteControlPairingInvitationCode): C.RemoteControlPairingInvitationCode {
  return { value: value.value };
}
export function liftRemoteControlControllerPairingResponseReceived(value: N.RemoteControlControllerPairingResponseReceived): C.RemoteControlControllerPairingResponseReceived {
  return { delivered: liftRemoteControlPacketReceiptDelivered(value.delivered), admission: liftRemoteControlAdmitRemoteControlControllerPairingResponseOutcome(value.admission), effect: liftRemoteControlControllerPairingResponseEffect(value.effect) };
}
export function liftRemoteControlPacketReceiptDelivered(value: N.RemoteControlPacketReceiptDelivered): C.RemoteControlPacketReceiptDelivered {
  return { rtt: liftRemoteControlRttMillis(value.rtt), evidence: liftRemoteControlDeliveryEvidence(value.evidence) };
}
export function liftRemoteControlRttMillis(value: N.RemoteControlRttMillis): C.RemoteControlRttMillis {
  return { value: value.value };
}
export function liftRemoteControlDeliveryEvidence(value: N.RemoteControlDeliveryEvidence): C.RemoteControlDeliveryEvidence {
  switch (value.tag) {
    case N.RemoteControlDeliveryEvidence_Tags.Proof: return { tag: "Proof", data: { value: liftRemoteControlDeliveryProof(value.inner.value) } };
    case N.RemoteControlDeliveryEvidence_Tags.Response: return { tag: "Response" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlDeliveryProof(value: N.RemoteControlDeliveryProof): C.RemoteControlDeliveryProof {
  switch (value.tag) {
    case N.RemoteControlDeliveryProof_Tags.Explicit: return { tag: "Explicit", data: { value: liftRemoteControlPacketHash(value.inner.value) } };
    case N.RemoteControlDeliveryProof_Tags.Implicit: return { tag: "Implicit", data: { value: liftRemoteControlPacketHash(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPacketHash(value: N.RemoteControlPacketHash): C.RemoteControlPacketHash {
  return { value: value.value };
}
export function liftRemoteControlAdmitRemoteControlControllerPairingResponseOutcome(value: N.RemoteControlAdmitRemoteControlControllerPairingResponseOutcome): C.RemoteControlAdmitRemoteControlControllerPairingResponseOutcome {
  switch (value.tag) {
    case N.RemoteControlAdmitRemoteControlControllerPairingResponseOutcome_Tags.NoActivePairing: return { tag: "NoActivePairing" };
    case N.RemoteControlAdmitRemoteControlControllerPairingResponseOutcome_Tags.UnrelatedLink: return { tag: "UnrelatedLink", data: { expected: liftRemoteControlLinkId(value.inner.expected), received: liftRemoteControlLinkId(value.inner.received) } };
    case N.RemoteControlAdmitRemoteControlControllerPairingResponseOutcome_Tags.MalformedEnvelope: return { tag: "MalformedEnvelope", data: { value: liftRemoteControlPackedBinaryParseError(value.inner.value) } };
    case N.RemoteControlAdmitRemoteControlControllerPairingResponseOutcome_Tags.MalformedResponse: return { tag: "MalformedResponse", data: { value: liftRemoteControlPairingMessageParseError(value.inner.value) } };
    case N.RemoteControlAdmitRemoteControlControllerPairingResponseOutcome_Tags.Offer: return { tag: "Offer", data: { value: liftRemoteControlReceiveRemoteControlControllerPairingOfferOutcome(value.inner.value) } };
    case N.RemoteControlAdmitRemoteControlControllerPairingResponseOutcome_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlReceiveRemoteControlControllerPairingCompletedOutcome(value.inner.value) } };
    case N.RemoteControlAdmitRemoteControlControllerPairingResponseOutcome_Tags.InvariantViolation: return { tag: "InvariantViolation", data: { value: liftRemoteControlControllerPairingResponseBridgeInvariantViolation(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPackedBinaryParseError(value: N.RemoteControlPackedBinaryParseError): C.RemoteControlPackedBinaryParseError {
  switch (value.tag) {
    case N.RemoteControlPackedBinaryParseError_Tags.Truncated: return { tag: "Truncated" };
    case N.RemoteControlPackedBinaryParseError_Tags.NotBinary: return { tag: "NotBinary" };
    case N.RemoteControlPackedBinaryParseError_Tags.LengthOutOfRange: return { tag: "LengthOutOfRange", data: { declared: value.inner.declared } };
    case N.RemoteControlPackedBinaryParseError_Tags.LengthMismatch: return { tag: "LengthMismatch", data: { declared: value.inner.declared, actual: value.inner.actual } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingMessageParseError(value: N.RemoteControlPairingMessageParseError): C.RemoteControlPairingMessageParseError {
  switch (value.tag) {
    case N.RemoteControlPairingMessageParseError_Tags.TooLong: return { tag: "TooLong", data: { actual: value.inner.actual, maximum: value.inner.maximum } };
    case N.RemoteControlPairingMessageParseError_Tags.Truncated: return { tag: "Truncated" };
    case N.RemoteControlPairingMessageParseError_Tags.UnsupportedVersion: return { tag: "UnsupportedVersion", data: { found: value.inner.found } };
    case N.RemoteControlPairingMessageParseError_Tags.UnknownKind: return { tag: "UnknownKind", data: { found: value.inner.found } };
    case N.RemoteControlPairingMessageParseError_Tags.UnexpectedKind: return { tag: "UnexpectedKind", data: { direction: liftRemoteControlPairingMessageDirection(value.inner.direction), found: liftRemoteControlPairingMessageKind(value.inner.found) } };
    case N.RemoteControlPairingMessageParseError_Tags.InvalidSigningPublicKey: return { tag: "InvalidSigningPublicKey", data: { role: liftRemoteControlPairingIdentityRole(value.inner.role) } };
    case N.RemoteControlPairingMessageParseError_Tags.UnknownRequestKind: return { tag: "UnknownRequestKind", data: { found: value.inner.found } };
    case N.RemoteControlPairingMessageParseError_Tags.UnknownAuthority: return { tag: "UnknownAuthority", data: { found: value.inner.found } };
    case N.RemoteControlPairingMessageParseError_Tags.TooManyPermissionsForVersion: return { tag: "TooManyPermissionsForVersion", data: { version: liftRemoteControlPairingProtocolVersion(value.inner.version), actual: value.inner.actual, maximum: value.inner.maximum } };
    case N.RemoteControlPairingMessageParseError_Tags.RequestUnsupportedForVersion: return { tag: "RequestUnsupportedForVersion", data: { version: liftRemoteControlPairingProtocolVersion(value.inner.version), request: liftRemoteControlRequestKind(value.inner.request) } };
    case N.RemoteControlPairingMessageParseError_Tags.NonCanonicalPermissions: return { tag: "NonCanonicalPermissions" };
    case N.RemoteControlPairingMessageParseError_Tags.InvalidPermissions: return { tag: "InvalidPermissions", data: { value: liftRemoteControlPairingPermissionsError(value.inner.value) } };
    case N.RemoteControlPairingMessageParseError_Tags.InvalidAttemptTimeout: return { tag: "InvalidAttemptTimeout", data: { value: liftRemoteControlPairingAttemptTimeoutError(value.inner.value) } };
    case N.RemoteControlPairingMessageParseError_Tags.TrailingBytes: return { tag: "TrailingBytes", data: { actual: value.inner.actual } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingMessageDirection(value: N.RemoteControlPairingMessageDirection): C.RemoteControlPairingMessageDirection {
  switch (value) {
    case N.RemoteControlPairingMessageDirection.Request: return { tag: "Request" };
    case N.RemoteControlPairingMessageDirection.Response: return { tag: "Response" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingMessageKind(value: N.RemoteControlPairingMessageKind): C.RemoteControlPairingMessageKind {
  switch (value) {
    case N.RemoteControlPairingMessageKind.Begin: return { tag: "Begin" };
    case N.RemoteControlPairingMessageKind.Offer: return { tag: "Offer" };
    case N.RemoteControlPairingMessageKind.Commit: return { tag: "Commit" };
    case N.RemoteControlPairingMessageKind.Completed: return { tag: "Completed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingIdentityRole(value: N.RemoteControlPairingIdentityRole): C.RemoteControlPairingIdentityRole {
  switch (value) {
    case N.RemoteControlPairingIdentityRole.Controller: return { tag: "Controller" };
    case N.RemoteControlPairingIdentityRole.Target: return { tag: "Target" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingProtocolVersion(value: N.RemoteControlPairingProtocolVersion): C.RemoteControlPairingProtocolVersion {
  switch (value) {
    case N.RemoteControlPairingProtocolVersion.V2: return { tag: "V2" };
    case N.RemoteControlPairingProtocolVersion.V3: return { tag: "V3" };
    case N.RemoteControlPairingProtocolVersion.V4: return { tag: "V4" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingPermissionsError(value: N.RemoteControlPairingPermissionsError): C.RemoteControlPairingPermissionsError {
  switch (value.tag) {
    case N.RemoteControlPairingPermissionsError_Tags.NoPermittedRequests: return { tag: "NoPermittedRequests" };
    case N.RemoteControlPairingPermissionsError_Tags.AdministratorRequestRequiresAuthority: return { tag: "AdministratorRequestRequiresAuthority", data: { request: liftRemoteControlRequestKind(value.inner.request) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingAttemptTimeoutError(value: N.RemoteControlPairingAttemptTimeoutError): C.RemoteControlPairingAttemptTimeoutError {
  switch (value.tag) {
    case N.RemoteControlPairingAttemptTimeoutError_Tags.Zero: return { tag: "Zero" };
    case N.RemoteControlPairingAttemptTimeoutError_Tags.TooLong: return { tag: "TooLong", data: { actual: liftRemoteControlDurationMillis(value.inner.actual), maximum: liftRemoteControlDurationMillis(value.inner.maximum) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlDurationMillis(value: N.RemoteControlDurationMillis): C.RemoteControlDurationMillis {
  return { value: value.value };
}
export function liftRemoteControlReceiveRemoteControlControllerPairingOfferOutcome(value: N.RemoteControlReceiveRemoteControlControllerPairingOfferOutcome): C.RemoteControlReceiveRemoteControlControllerPairingOfferOutcome {
  switch (value.tag) {
    case N.RemoteControlReceiveRemoteControlControllerPairingOfferOutcome_Tags.ConfirmationRequired: return { tag: "ConfirmationRequired", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlReceiveRemoteControlControllerPairingOfferOutcome_Tags.Expired: return { tag: "Expired", data: { expired: liftRemoteControlControllerPairingAborted(value.inner.expired) } };
    case N.RemoteControlReceiveRemoteControlControllerPairingOfferOutcome_Tags.Rejected: return { tag: "Rejected", data: { reason: liftRemoteControlPairingOfferVerificationError(value.inner.reason) } };
    case N.RemoteControlReceiveRemoteControlControllerPairingOfferOutcome_Tags.NoActiveAttempt: return { tag: "NoActiveAttempt" };
    case N.RemoteControlReceiveRemoteControlControllerPairingOfferOutcome_Tags.Unexpected: return { tag: "Unexpected", data: { active: liftRemoteControlControllerPairingActivity(value.inner.active) } };
    case N.RemoteControlReceiveRemoteControlControllerPairingOfferOutcome_Tags.PairingUnavailable: return { tag: "PairingUnavailable", data: { reason: liftRemoteControlControllerPairingAttemptWindowError(value.inner.reason) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingOfferVerificationError(value: N.RemoteControlPairingOfferVerificationError): C.RemoteControlPairingOfferVerificationError {
  switch (value.tag) {
    case N.RemoteControlPairingOfferVerificationError_Tags.ProtocolVersionMismatch: return { tag: "ProtocolVersionMismatch", data: { begin: liftRemoteControlPairingProtocolVersion(value.inner.begin), offer: liftRemoteControlPairingProtocolVersion(value.inner.offer) } };
    case N.RemoteControlPairingOfferVerificationError_Tags.InvalidTargetSignature: return { tag: "InvalidTargetSignature" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlControllerPairingActivity(value: N.RemoteControlControllerPairingActivity): C.RemoteControlControllerPairingActivity {
  switch (value.tag) {
    case N.RemoteControlControllerPairingActivity_Tags.AwaitingOffer: return { tag: "AwaitingOffer" };
    case N.RemoteControlControllerPairingActivity_Tags.AwaitingApproval: return { tag: "AwaitingApproval", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlControllerPairingActivity_Tags.AwaitingCompletion: return { tag: "AwaitingCompletion", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlControllerPairingActivity_Tags.Persisting: return { tag: "Persisting", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlControllerPairingAttemptWindowError(value: N.RemoteControlControllerPairingAttemptWindowError): C.RemoteControlControllerPairingAttemptWindowError {
  switch (value.tag) {
    case N.RemoteControlControllerPairingAttemptWindowError_Tags.PairingWindowElapsed: return { tag: "PairingWindowElapsed", data: { offeredAt: liftRemoteControlInstantMillis(value.inner.offeredAt), pairingExpiresAt: liftRemoteControlInstantMillis(value.inner.pairingExpiresAt) } };
    default: return unexpected(value.tag);
  }
}
export function liftRemoteControlReceiveRemoteControlControllerPairingCompletedOutcome(value: N.RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome): C.RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome {
  switch (value.tag) {
    case N.RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome_Tags.PersistenceOwed: return { tag: "PersistenceOwed", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome_Tags.Expired: return { tag: "Expired", data: { expired: liftRemoteControlControllerPairingAborted(value.inner.expired) } };
    case N.RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome_Tags.Rejected: return { tag: "Rejected", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), reason: liftRemoteControlPairingCompletedVerificationError(value.inner.reason) } };
    case N.RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome_Tags.NoActiveAttempt: return { tag: "NoActiveAttempt" };
    case N.RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome_Tags.OfferNotReceived: return { tag: "OfferNotReceived" };
    case N.RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome_Tags.ApprovalRequired: return { tag: "ApprovalRequired", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome_Tags.AlreadyReceived: return { tag: "AlreadyReceived", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingCompletedVerificationError(value: N.RemoteControlPairingCompletedVerificationError): C.RemoteControlPairingCompletedVerificationError {
  switch (value.tag) {
    case N.RemoteControlPairingCompletedVerificationError_Tags.ProtocolVersionMismatch: return { tag: "ProtocolVersionMismatch", data: { expected: liftRemoteControlPairingProtocolVersion(value.inner.expected), found: liftRemoteControlPairingProtocolVersion(value.inner.found) } };
    case N.RemoteControlPairingCompletedVerificationError_Tags.TranscriptMismatch: return { tag: "TranscriptMismatch", data: { expected: liftRemoteControlPairingTranscriptDigest(value.inner.expected), found: liftRemoteControlPairingTranscriptDigest(value.inner.found) } };
    case N.RemoteControlPairingCompletedVerificationError_Tags.InvalidTargetSignature: return { tag: "InvalidTargetSignature" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingTranscriptDigest(value: N.RemoteControlPairingTranscriptDigest): C.RemoteControlPairingTranscriptDigest {
  return { value: value.value };
}
export function liftRemoteControlControllerPairingResponseBridgeInvariantViolation(value: N.RemoteControlControllerPairingResponseBridgeInvariantViolation): C.RemoteControlControllerPairingResponseBridgeInvariantViolation {
  switch (value.tag) {
    case N.RemoteControlControllerPairingResponseBridgeInvariantViolation_Tags.ConfirmationStateUnavailable: return { tag: "ConfirmationStateUnavailable", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlControllerPairingResponseBridgeInvariantViolation_Tags.PersistenceStateUnavailable: return { tag: "PersistenceStateUnavailable", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlControllerPairingResponseEffect(value: N.RemoteControlControllerPairingResponseEffect): C.RemoteControlControllerPairingResponseEffect {
  switch (value.tag) {
    case N.RemoteControlControllerPairingResponseEffect_Tags.Advanced: return { tag: "Advanced" };
    case N.RemoteControlControllerPairingResponseEffect_Tags.Expired: return { tag: "Expired", data: { retiredLink: liftRemoteControlLinkId(value.inner.retiredLink) } };
    case N.RemoteControlControllerPairingResponseEffect_Tags.NotAdvanced: return { tag: "NotAdvanced", data: { value: liftRemoteControlFailRemoteControlControllerPairingRequestOutcome(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlFailRemoteControlControllerPairingRequestOutcome(value: N.RemoteControlFailRemoteControlControllerPairingRequestOutcome): C.RemoteControlFailRemoteControlControllerPairingRequestOutcome {
  switch (value.tag) {
    case N.RemoteControlFailRemoteControlControllerPairingRequestOutcome_Tags.Aborted: return { tag: "Aborted", data: { aborted: liftRemoteControlControllerPairingAborted(value.inner.aborted) } };
    case N.RemoteControlFailRemoteControlControllerPairingRequestOutcome_Tags.UnrelatedLink: return { tag: "UnrelatedLink" };
    case N.RemoteControlFailRemoteControlControllerPairingRequestOutcome_Tags.NoActiveAttempt: return { tag: "NoActiveAttempt" };
    case N.RemoteControlFailRemoteControlControllerPairingRequestOutcome_Tags.PersistenceInProgress: return { tag: "PersistenceInProgress", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure(value: N.RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure): C.RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure {
  switch (value.tag) {
    case N.RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlBeginRemoteControlControllerPairingControlFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlBeginRemoteControlControllerPairingControlFailure(value: N.RemoteControlBeginRemoteControlControllerPairingControlFailure): C.RemoteControlBeginRemoteControlControllerPairingControlFailure {
  switch (value.tag) {
    case N.RemoteControlBeginRemoteControlControllerPairingControlFailure_Tags.Begin: return { tag: "Begin", data: { value: liftRemoteControlBeginRemoteControlControllerPairingFailure(value.inner.value) } };
    case N.RemoteControlBeginRemoteControlControllerPairingControlFailure_Tags.Identify: return { tag: "Identify", data: { failure: liftRemoteControlSendErrorRemoteControlIdentifyFailure(value.inner.failure), cleanup: liftRemoteControlPairingLinkCleanupOutcome(value.inner.cleanup) } };
    case N.RemoteControlBeginRemoteControlControllerPairingControlFailure_Tags.Request: return { tag: "Request", data: { value: liftRemoteControlControllerPairingRequestFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlBeginRemoteControlControllerPairingFailure(value: N.RemoteControlBeginRemoteControlControllerPairingFailure): C.RemoteControlBeginRemoteControlControllerPairingFailure {
  switch (value.tag) {
    case N.RemoteControlBeginRemoteControlControllerPairingFailure_Tags.ControllerIdentityUnavailable: return { tag: "ControllerIdentityUnavailable" };
    case N.RemoteControlBeginRemoteControlControllerPairingFailure_Tags.Busy: return { tag: "Busy", data: { active: liftRemoteControlControllerPairingActivity(value.inner.active) } };
    case N.RemoteControlBeginRemoteControlControllerPairingFailure_Tags.PairingUnavailable: return { tag: "PairingUnavailable", data: { reason: liftRemoteControlControllerPairingWindowError(value.inner.reason) } };
    case N.RemoteControlBeginRemoteControlControllerPairingFailure_Tags.RequestBuild: return { tag: "RequestBuild", data: { failure: liftRemoteControlControllerPairingRequestBuildError(value.inner.failure), rollback: liftRemoteControlFailRemoteControlControllerPairingRequestOutcome(value.inner.rollback) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlControllerPairingWindowError(value: N.RemoteControlControllerPairingWindowError): C.RemoteControlControllerPairingWindowError {
  switch (value.tag) {
    case N.RemoteControlControllerPairingWindowError_Tags.DeadlineNotFuture: return { tag: "DeadlineNotFuture", data: { startedAt: liftRemoteControlInstantMillis(value.inner.startedAt), expiresAt: liftRemoteControlInstantMillis(value.inner.expiresAt) } };
    default: return unexpected(value.tag);
  }
}
export function liftRemoteControlControllerPairingRequestBuildError(value: N.RemoteControlControllerPairingRequestBuildError): C.RemoteControlControllerPairingRequestBuildError {
  switch (value.tag) {
    case N.RemoteControlControllerPairingRequestBuildError_Tags.Encode: return { tag: "Encode", data: { value: liftRemoteControlPairingMessageWriteError(value.inner.value) } };
    case N.RemoteControlControllerPairingRequestBuildError_Tags.Pack: return { tag: "Pack", data: { value: liftRemoteControlPackBinaryError(value.inner.value) } };
    case N.RemoteControlControllerPairingRequestBuildError_Tags.Capacity: return { tag: "Capacity", data: { required: value.inner.required, maximum: value.inner.maximum } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingMessageWriteError(value: N.RemoteControlPairingMessageWriteError): C.RemoteControlPairingMessageWriteError {
  switch (value.tag) {
    case N.RemoteControlPairingMessageWriteError_Tags.BufferTooShort: return { tag: "BufferTooShort", data: { required: value.inner.required, actual: value.inner.actual } };
    default: return unexpected(value.tag);
  }
}
export function liftRemoteControlPackBinaryError(value: N.RemoteControlPackBinaryError): C.RemoteControlPackBinaryError {
  switch (value) {
    case N.RemoteControlPackBinaryError.BufferTooShort: return { tag: "BufferTooShort" };
    case N.RemoteControlPackBinaryError.LengthOutOfRange: return { tag: "LengthOutOfRange" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingLinkCleanupOutcome(value: N.RemoteControlPairingLinkCleanupOutcome): C.RemoteControlPairingLinkCleanupOutcome {
  switch (value) {
    case N.RemoteControlPairingLinkCleanupOutcome.Queued: return { tag: "Queued" };
    case N.RemoteControlPairingLinkCleanupOutcome.NotQueued: return { tag: "NotQueued" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlControllerPairingRequestFailure(value: N.RemoteControlControllerPairingRequestFailure): C.RemoteControlControllerPairingRequestFailure {
  return { cause: liftRemoteControlControllerPairingRequestFailureCause(value.cause), exchange: liftRemoteControlFailRemoteControlControllerPairingRequestOutcome(value.exchange) };
}
export function liftRemoteControlControllerPairingRequestFailureCause(value: N.RemoteControlControllerPairingRequestFailureCause): C.RemoteControlControllerPairingRequestFailureCause {
  switch (value.tag) {
    case N.RemoteControlControllerPairingRequestFailureCause_Tags.Request: return { tag: "Request", data: { value: liftRemoteControlSendRequestFailure(value.inner.value) } };
    case N.RemoteControlControllerPairingRequestFailureCause_Tags.ResourceResponseUnsupported: return { tag: "ResourceResponseUnsupported" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlBeginRemoteControlControllerPairingControlError(value: N.RemoteControlBeginRemoteControlControllerPairingControlError): C.RemoteControlBeginRemoteControlControllerPairingControlError {
  switch (value.tag) {
    case N.RemoteControlBeginRemoteControlControllerPairingControlError_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlBeginRemoteControlControllerPairingControlError_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlBeginRemoteControlControllerPairingControlError_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlBeginRemoteControlControllerPairingControlFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlBeginRemoteControlControllerPairingSettlement(value: N.RemoteControlBeginRemoteControlControllerPairingSettlement): C.RemoteControlBeginRemoteControlControllerPairingSettlement {
  switch (value.tag) {
    case N.RemoteControlBeginRemoteControlControllerPairingSettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlControllerPairingResponseReceived(value.inner.value) } };
    case N.RemoteControlBeginRemoteControlControllerPairingSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlBeginRemoteControlControllerPairingControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function lowerRemoteControlApproveRemoteControlControllerPairing(value: C.RemoteControlApproveRemoteControlControllerPairing): N.RemoteControlApproveRemoteControlControllerPairing {
  return { attemptId: lowerRemoteControlPairingAttemptId(value.attemptId) };
}
export function liftRemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure(value: N.RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure): C.RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure {
  switch (value.tag) {
    case N.RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlApproveRemoteControlControllerPairingControlFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlApproveRemoteControlControllerPairingControlFailure(value: N.RemoteControlApproveRemoteControlControllerPairingControlFailure): C.RemoteControlApproveRemoteControlControllerPairingControlFailure {
  switch (value.tag) {
    case N.RemoteControlApproveRemoteControlControllerPairingControlFailure_Tags.Approve: return { tag: "Approve", data: { value: liftRemoteControlApproveRemoteControlControllerPairingFailure(value.inner.value) } };
    case N.RemoteControlApproveRemoteControlControllerPairingControlFailure_Tags.Request: return { tag: "Request", data: { value: liftRemoteControlControllerPairingRequestFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlApproveRemoteControlControllerPairingFailure(value: N.RemoteControlApproveRemoteControlControllerPairingFailure): C.RemoteControlApproveRemoteControlControllerPairingFailure {
  switch (value.tag) {
    case N.RemoteControlApproveRemoteControlControllerPairingFailure_Tags.Expired: return { tag: "Expired", data: { expired: liftRemoteControlControllerPairingAborted(value.inner.expired), retiredLink: liftRemoteControlLinkId(value.inner.retiredLink) } };
    case N.RemoteControlApproveRemoteControlControllerPairingFailure_Tags.NoActiveAttempt: return { tag: "NoActiveAttempt" };
    case N.RemoteControlApproveRemoteControlControllerPairingFailure_Tags.OfferNotReceived: return { tag: "OfferNotReceived" };
    case N.RemoteControlApproveRemoteControlControllerPairingFailure_Tags.AttemptMismatch: return { tag: "AttemptMismatch", data: { requested: liftRemoteControlPairingAttemptId(value.inner.requested), active: liftRemoteControlPairingAttemptId(value.inner.active) } };
    case N.RemoteControlApproveRemoteControlControllerPairingFailure_Tags.PersistenceInProgress: return { tag: "PersistenceInProgress", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlApproveRemoteControlControllerPairingFailure_Tags.RequestBuild: return { tag: "RequestBuild", data: { failure: liftRemoteControlControllerPairingRequestBuildError(value.inner.failure), rollback: liftRemoteControlFailRemoteControlControllerPairingRequestOutcome(value.inner.rollback) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlApproveRemoteControlControllerPairingControlError(value: N.RemoteControlApproveRemoteControlControllerPairingControlError): C.RemoteControlApproveRemoteControlControllerPairingControlError {
  switch (value.tag) {
    case N.RemoteControlApproveRemoteControlControllerPairingControlError_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlApproveRemoteControlControllerPairingControlError_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlApproveRemoteControlControllerPairingControlError_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlApproveRemoteControlControllerPairingControlFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlApproveRemoteControlControllerPairingSettlement(value: N.RemoteControlApproveRemoteControlControllerPairingSettlement): C.RemoteControlApproveRemoteControlControllerPairingSettlement {
  switch (value.tag) {
    case N.RemoteControlApproveRemoteControlControllerPairingSettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlControllerPairingResponseReceived(value.inner.value) } };
    case N.RemoteControlApproveRemoteControlControllerPairingSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlApproveRemoteControlControllerPairingControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function lowerRemoteControlRejectRemoteControlControllerPairing(value: C.RemoteControlRejectRemoteControlControllerPairing): N.RemoteControlRejectRemoteControlControllerPairing {
  return { attemptId: lowerRemoteControlPairingAttemptId(value.attemptId) };
}
export function liftRemoteControlControllerPairingRejection(value: N.RemoteControlControllerPairingRejection): C.RemoteControlControllerPairingRejection {
  return { aborted: liftRemoteControlControllerPairingAborted(value.aborted), retiredLink: liftRemoteControlLinkId(value.retiredLink) };
}
export function liftRemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure(value: N.RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure): C.RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure {
  switch (value.tag) {
    case N.RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlRejectRemoteControlControllerPairingFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlRejectRemoteControlControllerPairingFailure(value: N.RemoteControlRejectRemoteControlControllerPairingFailure): C.RemoteControlRejectRemoteControlControllerPairingFailure {
  switch (value.tag) {
    case N.RemoteControlRejectRemoteControlControllerPairingFailure_Tags.Expired: return { tag: "Expired", data: { expired: liftRemoteControlControllerPairingAborted(value.inner.expired), retiredLink: liftRemoteControlLinkId(value.inner.retiredLink) } };
    case N.RemoteControlRejectRemoteControlControllerPairingFailure_Tags.NoActiveAttempt: return { tag: "NoActiveAttempt" };
    case N.RemoteControlRejectRemoteControlControllerPairingFailure_Tags.OfferNotReceived: return { tag: "OfferNotReceived" };
    case N.RemoteControlRejectRemoteControlControllerPairingFailure_Tags.AttemptMismatch: return { tag: "AttemptMismatch", data: { requested: liftRemoteControlPairingAttemptId(value.inner.requested), active: liftRemoteControlPairingAttemptId(value.inner.active) } };
    case N.RemoteControlRejectRemoteControlControllerPairingFailure_Tags.AlreadyApproved: return { tag: "AlreadyApproved", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlRejectRemoteControlControllerPairingFailure_Tags.PersistenceInProgress: return { tag: "PersistenceInProgress", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlRejectRemoteControlControllerPairingControlError(value: N.RemoteControlRejectRemoteControlControllerPairingControlError): C.RemoteControlRejectRemoteControlControllerPairingControlError {
  switch (value.tag) {
    case N.RemoteControlRejectRemoteControlControllerPairingControlError_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlRejectRemoteControlControllerPairingControlError_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlRejectRemoteControlControllerPairingControlError_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlRejectRemoteControlControllerPairingFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlRejectRemoteControlControllerPairingSettlement(value: N.RemoteControlRejectRemoteControlControllerPairingSettlement): C.RemoteControlRejectRemoteControlControllerPairingSettlement {
  switch (value.tag) {
    case N.RemoteControlRejectRemoteControlControllerPairingSettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlControllerPairingRejection(value.inner.value) } };
    case N.RemoteControlRejectRemoteControlControllerPairingSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlRejectRemoteControlControllerPairingControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function lowerRemoteControlApproveRemoteControlTargetPairing(value: C.RemoteControlApproveRemoteControlTargetPairing): N.RemoteControlApproveRemoteControlTargetPairing {
  return { attemptId: lowerRemoteControlPairingAttemptId(value.attemptId) };
}
export function liftRemoteControlTargetPairingApproval(value: N.RemoteControlTargetPairingApproval): C.RemoteControlTargetPairingApproval {
  switch (value.tag) {
    case N.RemoteControlTargetPairingApproval_Tags.AwaitingControllerCommit: return { tag: "AwaitingControllerCommit", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlTargetPairingApproval_Tags.AuthorizationOwed: return { tag: "AuthorizationOwed", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), grant: liftRemoteControlControllerGrant(value.inner.grant) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure(value: N.RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure): C.RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure {
  switch (value.tag) {
    case N.RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlApproveRemoteControlTargetPairingFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlApproveRemoteControlTargetPairingFailure(value: N.RemoteControlApproveRemoteControlTargetPairingFailure): C.RemoteControlApproveRemoteControlTargetPairingFailure {
  switch (value.tag) {
    case N.RemoteControlApproveRemoteControlTargetPairingFailure_Tags.Expired: return { tag: "Expired", data: { expired: liftRemoteControlTargetPairingAborted(value.inner.expired), retiredLink: liftRemoteControlLinkId(value.inner.retiredLink) } };
    case N.RemoteControlApproveRemoteControlTargetPairingFailure_Tags.NoActiveAttempt: return { tag: "NoActiveAttempt" };
    case N.RemoteControlApproveRemoteControlTargetPairingFailure_Tags.AttemptMismatch: return { tag: "AttemptMismatch", data: { requested: liftRemoteControlPairingAttemptId(value.inner.requested), active: liftRemoteControlPairingAttemptId(value.inner.active) } };
    case N.RemoteControlApproveRemoteControlTargetPairingFailure_Tags.OfferPendingDispatch: return { tag: "OfferPendingDispatch", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlApproveRemoteControlTargetPairingFailure_Tags.AlreadyApproved: return { tag: "AlreadyApproved", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlApproveRemoteControlTargetPairingFailure_Tags.FinalizationInProgress: return { tag: "FinalizationInProgress", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlApproveRemoteControlTargetPairingFailure_Tags.CompletionRetentionExpired: return { tag: "CompletionRetentionExpired", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), retiredLink: liftRemoteControlLinkId(value.inner.retiredLink) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlApproveRemoteControlTargetPairingControlError(value: N.RemoteControlApproveRemoteControlTargetPairingControlError): C.RemoteControlApproveRemoteControlTargetPairingControlError {
  switch (value.tag) {
    case N.RemoteControlApproveRemoteControlTargetPairingControlError_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlApproveRemoteControlTargetPairingControlError_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlApproveRemoteControlTargetPairingControlError_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlApproveRemoteControlTargetPairingFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlApproveRemoteControlTargetPairingSettlement(value: N.RemoteControlApproveRemoteControlTargetPairingSettlement): C.RemoteControlApproveRemoteControlTargetPairingSettlement {
  switch (value.tag) {
    case N.RemoteControlApproveRemoteControlTargetPairingSettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlTargetPairingApproval(value.inner.value) } };
    case N.RemoteControlApproveRemoteControlTargetPairingSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlApproveRemoteControlTargetPairingControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function lowerRemoteControlRejectRemoteControlTargetPairing(value: C.RemoteControlRejectRemoteControlTargetPairing): N.RemoteControlRejectRemoteControlTargetPairing {
  return { attemptId: lowerRemoteControlPairingAttemptId(value.attemptId) };
}
export function liftRemoteControlTargetPairingRejection(value: N.RemoteControlTargetPairingRejection): C.RemoteControlTargetPairingRejection {
  return { aborted: liftRemoteControlTargetPairingAborted(value.aborted), retiredLink: liftRemoteControlLinkId(value.retiredLink) };
}
export function liftRemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure(value: N.RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure): C.RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure {
  switch (value.tag) {
    case N.RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlRejectRemoteControlTargetPairingFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlRejectRemoteControlTargetPairingFailure(value: N.RemoteControlRejectRemoteControlTargetPairingFailure): C.RemoteControlRejectRemoteControlTargetPairingFailure {
  switch (value.tag) {
    case N.RemoteControlRejectRemoteControlTargetPairingFailure_Tags.Expired: return { tag: "Expired", data: { expired: liftRemoteControlTargetPairingAborted(value.inner.expired), retiredLink: liftRemoteControlLinkId(value.inner.retiredLink) } };
    case N.RemoteControlRejectRemoteControlTargetPairingFailure_Tags.NoActiveAttempt: return { tag: "NoActiveAttempt" };
    case N.RemoteControlRejectRemoteControlTargetPairingFailure_Tags.AttemptMismatch: return { tag: "AttemptMismatch", data: { requested: liftRemoteControlPairingAttemptId(value.inner.requested), active: liftRemoteControlPairingAttemptId(value.inner.active) } };
    case N.RemoteControlRejectRemoteControlTargetPairingFailure_Tags.OfferPendingDispatch: return { tag: "OfferPendingDispatch", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlRejectRemoteControlTargetPairingFailure_Tags.AlreadyApproved: return { tag: "AlreadyApproved", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlRejectRemoteControlTargetPairingFailure_Tags.FinalizationInProgress: return { tag: "FinalizationInProgress", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId) } };
    case N.RemoteControlRejectRemoteControlTargetPairingFailure_Tags.CompletionRetentionExpired: return { tag: "CompletionRetentionExpired", data: { attemptId: liftRemoteControlPairingAttemptId(value.inner.attemptId), retiredLink: liftRemoteControlLinkId(value.inner.retiredLink) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlRejectRemoteControlTargetPairingControlError(value: N.RemoteControlRejectRemoteControlTargetPairingControlError): C.RemoteControlRejectRemoteControlTargetPairingControlError {
  switch (value.tag) {
    case N.RemoteControlRejectRemoteControlTargetPairingControlError_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlRejectRemoteControlTargetPairingControlError_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlRejectRemoteControlTargetPairingControlError_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlRejectRemoteControlTargetPairingFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlRejectRemoteControlTargetPairingSettlement(value: N.RemoteControlRejectRemoteControlTargetPairingSettlement): C.RemoteControlRejectRemoteControlTargetPairingSettlement {
  switch (value.tag) {
    case N.RemoteControlRejectRemoteControlTargetPairingSettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlTargetPairingRejection(value.inner.value) } };
    case N.RemoteControlRejectRemoteControlTargetPairingSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlRejectRemoteControlTargetPairingControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlTargetInventory(value: N.RemoteControlTargetInventory): C.RemoteControlTargetInventory {
  return { targets: value.targets.map(item => liftRemoteControlIdentityHash(item)) };
}
export function liftRemoteControlTargetInventoryControlError(value: N.RemoteControlTargetInventoryControlError): C.RemoteControlTargetInventoryControlError {
  switch (value.tag) {
    case N.RemoteControlTargetInventoryControlError_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlTargetInventoryControlError_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlTargetInventoryControlError_Tags.Unavailable: return { tag: "Unavailable" };
    case N.RemoteControlTargetInventoryControlError_Tags.Inventory: return { tag: "Inventory", data: { value: liftRemoteControlTargetInventoryError(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlTargetInventoryError(value: N.RemoteControlTargetInventoryError): C.RemoteControlTargetInventoryError {
  switch (value) {
    case N.RemoteControlTargetInventoryError.CapacityInvariantViolation: return { tag: "CapacityInvariantViolation" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlTargetInventorySettlement(value: N.RemoteControlTargetInventorySettlement): C.RemoteControlTargetInventorySettlement {
  switch (value.tag) {
    case N.RemoteControlTargetInventorySettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlTargetInventory(value.inner.value) } };
    case N.RemoteControlTargetInventorySettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlTargetInventoryControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlResolvedRemoteControlTarget(value: N.RemoteControlResolvedRemoteControlTarget): C.RemoteControlResolvedRemoteControlTarget {
  return { target: liftRemoteControlIdentityHash(value.target), endpoint: liftRemoteControlEndpoint(value.endpoint), controller: liftRemoteControlControllerIdentity(value.controller), permittedRequests: liftRemoteControlRequestSet(value.permittedRequests) };
}
export function liftRemoteControlEndpoint(value: N.RemoteControlEndpoint): C.RemoteControlEndpoint {
  return { destinationHash: liftRemoteControlDestinationHash(value.destinationHash) };
}
export function liftRemoteControlResolveRemoteControlTargetSettlement(value: N.RemoteControlResolveRemoteControlTargetSettlement): C.RemoteControlResolveRemoteControlTargetSettlement {
  switch (value.tag) {
    case N.RemoteControlResolveRemoteControlTargetSettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlResolvedRemoteControlTarget(value.inner.value) } };
    case N.RemoteControlResolveRemoteControlTargetSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlResolveRemoteControlTargetControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function lowerRemoteControlTargetAccess(value: C.RemoteControlTargetAccess): N.RemoteControlTargetAccess {
  return { target: lowerRemoteControlTargetIdentity(value.target), authority: lowerRemoteControlControllerAuthority(value.authority), permittedRequests: lowerRemoteControlRequestSet(value.permittedRequests) };
}
export function liftRemoteControlTargetAccess(value: N.RemoteControlTargetAccess): C.RemoteControlTargetAccess {
  return { target: liftRemoteControlTargetIdentity(value.target), authority: liftRemoteControlControllerAuthority(value.authority), permittedRequests: liftRemoteControlRequestSet(value.permittedRequests) };
}
export function liftRemoteControlSetRemoteControlTargetAccessOutcome(value: N.RemoteControlSetRemoteControlTargetAccessOutcome): C.RemoteControlSetRemoteControlTargetAccessOutcome {
  switch (value.tag) {
    case N.RemoteControlSetRemoteControlTargetAccessOutcome_Tags.Added: return { tag: "Added" };
    case N.RemoteControlSetRemoteControlTargetAccessOutcome_Tags.Unchanged: return { tag: "Unchanged" };
    case N.RemoteControlSetRemoteControlTargetAccessOutcome_Tags.Updated: return { tag: "Updated", data: { previous: liftRemoteControlTargetAccess(value.inner.previous) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlSetRemoteControlTargetAccessControlError(value: N.RemoteControlSetRemoteControlTargetAccessControlError): C.RemoteControlSetRemoteControlTargetAccessControlError {
  switch (value) {
    case N.RemoteControlSetRemoteControlTargetAccessControlError.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlSetRemoteControlTargetAccessControlError.Busy: return { tag: "Busy" };
    case N.RemoteControlSetRemoteControlTargetAccessControlError.Unavailable: return { tag: "Unavailable" };
    case N.RemoteControlSetRemoteControlTargetAccessControlError.CapacityExhausted: return { tag: "CapacityExhausted" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlSetRemoteControlTargetAccessSettlement(value: N.RemoteControlSetRemoteControlTargetAccessSettlement): C.RemoteControlSetRemoteControlTargetAccessSettlement {
  switch (value.tag) {
    case N.RemoteControlSetRemoteControlTargetAccessSettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlSetRemoteControlTargetAccessOutcome(value.inner.value) } };
    case N.RemoteControlSetRemoteControlTargetAccessSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlSetRemoteControlTargetAccessControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlForgetRemoteControlTargetOutcome(value: N.RemoteControlForgetRemoteControlTargetOutcome): C.RemoteControlForgetRemoteControlTargetOutcome {
  switch (value.tag) {
    case N.RemoteControlForgetRemoteControlTargetOutcome_Tags.Forgotten: return { tag: "Forgotten", data: { access: liftRemoteControlTargetAccess(value.inner.access) } };
    case N.RemoteControlForgetRemoteControlTargetOutcome_Tags.NotFound: return { tag: "NotFound" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlForgetRemoteControlTargetControlError(value: N.RemoteControlForgetRemoteControlTargetControlError): C.RemoteControlForgetRemoteControlTargetControlError {
  switch (value) {
    case N.RemoteControlForgetRemoteControlTargetControlError.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlForgetRemoteControlTargetControlError.Busy: return { tag: "Busy" };
    case N.RemoteControlForgetRemoteControlTargetControlError.Unavailable: return { tag: "Unavailable" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlForgetRemoteControlTargetSettlement(value: N.RemoteControlForgetRemoteControlTargetSettlement): C.RemoteControlForgetRemoteControlTargetSettlement {
  switch (value.tag) {
    case N.RemoteControlForgetRemoteControlTargetSettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlForgetRemoteControlTargetOutcome(value.inner.value) } };
    case N.RemoteControlForgetRemoteControlTargetSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlForgetRemoteControlTargetControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function lowerRemoteControlOpenRemoteControlPairing(value: C.RemoteControlOpenRemoteControlPairing): N.RemoteControlOpenRemoteControlPairing {
  return { target: lowerRemoteControlEgressTarget(value.target), expiresAfter: lowerRemoteControlPairingExpiresAfter(value.expiresAfter), attemptTimeout: lowerRemoteControlPairingAttemptTimeout(value.attemptTimeout), permissions: lowerRemoteControlPairingPermissions(value.permissions), publicAppData: lowerRemoteControlPairingPublicAppDataBytes(value.publicAppData) };
}
export function lowerRemoteControlEgressTarget(value: C.RemoteControlEgressTarget): N.RemoteControlEgressTarget {
  switch (value.tag) {
    case "AllInterfaces": return N.RemoteControlEgressTarget.AllInterfaces.new();
    case "Interface": return N.RemoteControlEgressTarget.Interface.new({ value: lowerRemoteControlInterfaceId(value.data.value) });
    default: return unexpected(value);
  }
}
export function lowerRemoteControlPairingExpiresAfter(value: C.RemoteControlPairingExpiresAfter): N.RemoteControlPairingExpiresAfter {
  return { value: value.value };
}
export function liftRemoteControlPairingExpiresAfter(value: N.RemoteControlPairingExpiresAfter): C.RemoteControlPairingExpiresAfter {
  return { value: value.value };
}
export function lowerRemoteControlPairingPublicAppDataBytes(value: C.RemoteControlPairingPublicAppDataBytes): N.RemoteControlPairingPublicAppDataBytes {
  return { value: value.value };
}
export function liftRemoteControlPairingOpened(value: N.RemoteControlPairingOpened): C.RemoteControlPairingOpened {
  return { endpoint: liftRemoteControlPairingEndpoint(value.endpoint), expiresAt: liftRemoteControlInstantMillis(value.expiresAt), invitationCode: liftRemoteControlPairingInvitationCode(value.invitationCode) };
}
export function liftRemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure(value: N.RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure): C.RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure {
  switch (value.tag) {
    case N.RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlOpenRemoteControlPairingFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlOpenRemoteControlPairingFailure(value: N.RemoteControlOpenRemoteControlPairingFailure): C.RemoteControlOpenRemoteControlPairingFailure {
  switch (value.tag) {
    case N.RemoteControlOpenRemoteControlPairingFailure_Tags.Rejected: return { tag: "Rejected", data: { value: liftRemoteControlOpenRemoteControlPairingRejection(value.inner.value) } };
    case N.RemoteControlOpenRemoteControlPairingFailure_Tags.IdentityGenerationExhausted: return { tag: "IdentityGenerationExhausted" };
    case N.RemoteControlOpenRemoteControlPairingFailure_Tags.HoldIdentity: return { tag: "HoldIdentity", data: { value: liftRemoteControlHoldIdentityError(value.inner.value) } };
    case N.RemoteControlOpenRemoteControlPairingFailure_Tags.RegisterEndpoint: return { tag: "RegisterEndpoint", data: { value: liftRemoteControlRegisterDestinationError(value.inner.value) } };
    case N.RemoteControlOpenRemoteControlPairingFailure_Tags.ConfigureRequestLimit: return { tag: "ConfigureRequestLimit" };
    case N.RemoteControlOpenRemoteControlPairingFailure_Tags.RegisterRequestEndpoint: return { tag: "RegisterRequestEndpoint", data: { value: liftRemoteControlTablePushError(value.inner.value) } };
    case N.RemoteControlOpenRemoteControlPairingFailure_Tags.WriteAvailability: return { tag: "WriteAvailability", data: { value: liftRemoteControlPairingAvailabilityWriteError(value.inner.value) } };
    case N.RemoteControlOpenRemoteControlPairingFailure_Tags.PayloadCapacity: return { tag: "PayloadCapacity" };
    case N.RemoteControlOpenRemoteControlPairingFailure_Tags.WritePacket: return { tag: "WritePacket", data: { value: liftRemoteControlSendPlainPacketWriteError(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlOpenRemoteControlPairingRejection(value: N.RemoteControlOpenRemoteControlPairingRejection): C.RemoteControlOpenRemoteControlPairingRejection {
  switch (value.tag) {
    case N.RemoteControlOpenRemoteControlPairingRejection_Tags.Unavailable: return { tag: "Unavailable" };
    case N.RemoteControlOpenRemoteControlPairingRejection_Tags.AlreadyOpen: return { tag: "AlreadyOpen" };
    case N.RemoteControlOpenRemoteControlPairingRejection_Tags.NoTransmittingInterfaces: return { tag: "NoTransmittingInterfaces" };
    case N.RemoteControlOpenRemoteControlPairingRejection_Tags.EgressTarget: return { tag: "EgressTarget", data: { value: liftRemoteControlEgressTargetRejection(value.inner.value) } };
    case N.RemoteControlOpenRemoteControlPairingRejection_Tags.AttemptTimeoutExceedsWindow: return { tag: "AttemptTimeoutExceedsWindow", data: { attemptTimeout: liftRemoteControlPairingAttemptTimeout(value.inner.attemptTimeout), pairingExpiresAfter: liftRemoteControlPairingExpiresAfter(value.inner.pairingExpiresAfter) } };
    case N.RemoteControlOpenRemoteControlPairingRejection_Tags.DeadlineOverflow: return { tag: "DeadlineOverflow" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlEgressTargetRejection(value: N.RemoteControlEgressTargetRejection): C.RemoteControlEgressTargetRejection {
  switch (value) {
    case N.RemoteControlEgressTargetRejection.UnknownInterface: return { tag: "UnknownInterface" };
    case N.RemoteControlEgressTargetRejection.InterfaceCannotTransmit: return { tag: "InterfaceCannotTransmit" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlHoldIdentityError(value: N.RemoteControlHoldIdentityError): C.RemoteControlHoldIdentityError {
  switch (value) {
    case N.RemoteControlHoldIdentityError.StoreFull: return { tag: "StoreFull" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlRegisterDestinationError(value: N.RemoteControlRegisterDestinationError): C.RemoteControlRegisterDestinationError {
  switch (value.tag) {
    case N.RemoteControlRegisterDestinationError_Tags.Name: return { tag: "Name", data: { value: liftRemoteControlExpandNameError(value.inner.value) } };
    case N.RemoteControlRegisterDestinationError_Tags.RegistryFull: return { tag: "RegistryFull" };
    case N.RemoteControlRegisterDestinationError_Tags.UnknownIdentity: return { tag: "UnknownIdentity" };
    case N.RemoteControlRegisterDestinationError_Tags.RatchetTableFull: return { tag: "RatchetTableFull" };
    case N.RemoteControlRegisterDestinationError_Tags.AppDataTooLong: return { tag: "AppDataTooLong" };
    case N.RemoteControlRegisterDestinationError_Tags.InvalidGroupKey: return { tag: "InvalidGroupKey" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlExpandNameError(value: N.RemoteControlExpandNameError): C.RemoteControlExpandNameError {
  switch (value) {
    case N.RemoteControlExpandNameError.DotInComponent: return { tag: "DotInComponent" };
    case N.RemoteControlExpandNameError.NameTooLong: return { tag: "NameTooLong" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlTablePushError(value: N.RemoteControlTablePushError): C.RemoteControlTablePushError {
  switch (value) {
    case N.RemoteControlTablePushError.TableFull: return { tag: "TableFull" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingAvailabilityWriteError(value: N.RemoteControlPairingAvailabilityWriteError): C.RemoteControlPairingAvailabilityWriteError {
  switch (value.tag) {
    case N.RemoteControlPairingAvailabilityWriteError_Tags.BufferTooShort: return { tag: "BufferTooShort", data: { required: value.inner.required, actual: value.inner.actual } };
    case N.RemoteControlPairingAvailabilityWriteError_Tags.BuildAnnounce: return { tag: "BuildAnnounce", data: { value: liftRemoteControlAnnounceBuildError(value.inner.value) } };
    case N.RemoteControlPairingAvailabilityWriteError_Tags.WriteAnnounce: return { tag: "WriteAnnounce", data: { value: liftRemoteControlWireError(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlAnnounceBuildError(value: N.RemoteControlAnnounceBuildError): C.RemoteControlAnnounceBuildError {
  switch (value) {
    case N.RemoteControlAnnounceBuildError.AnnounceTooLarge: return { tag: "AnnounceTooLarge" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlWireError(value: N.RemoteControlWireError): C.RemoteControlWireError {
  switch (value) {
    case N.RemoteControlWireError.BufferTooShort: return { tag: "BufferTooShort" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlSendPlainPacketWriteError(value: N.RemoteControlSendPlainPacketWriteError): C.RemoteControlSendPlainPacketWriteError {
  switch (value) {
    case N.RemoteControlSendPlainPacketWriteError.Serialize: return { tag: "Serialize" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlOpenRemoteControlPairingControlError(value: N.RemoteControlOpenRemoteControlPairingControlError): C.RemoteControlOpenRemoteControlPairingControlError {
  switch (value.tag) {
    case N.RemoteControlOpenRemoteControlPairingControlError_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlOpenRemoteControlPairingControlError_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlOpenRemoteControlPairingControlError_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlOpenRemoteControlPairingFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlOpenRemoteControlPairingSettlement(value: N.RemoteControlOpenRemoteControlPairingSettlement): C.RemoteControlOpenRemoteControlPairingSettlement {
  switch (value.tag) {
    case N.RemoteControlOpenRemoteControlPairingSettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlPairingOpened(value.inner.value) } };
    case N.RemoteControlOpenRemoteControlPairingSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlOpenRemoteControlPairingControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlCloseRemoteControlPairingOutcome(value: N.RemoteControlCloseRemoteControlPairingOutcome): C.RemoteControlCloseRemoteControlPairingOutcome {
  switch (value.tag) {
    case N.RemoteControlCloseRemoteControlPairingOutcome_Tags.Closed: return { tag: "Closed", data: { endpoint: liftRemoteControlPairingEndpoint(value.inner.endpoint) } };
    case N.RemoteControlCloseRemoteControlPairingOutcome_Tags.AlreadyClosed: return { tag: "AlreadyClosed" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure(value: N.RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure): C.RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure {
  switch (value.tag) {
    case N.RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlCloseRemoteControlPairingFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlCloseRemoteControlPairingFailure(value: N.RemoteControlCloseRemoteControlPairingFailure): C.RemoteControlCloseRemoteControlPairingFailure {
  switch (value.tag) {
    case N.RemoteControlCloseRemoteControlPairingFailure_Tags.Unavailable: return { tag: "Unavailable" };
    case N.RemoteControlCloseRemoteControlPairingFailure_Tags.RetirementIncomplete: return { tag: "RetirementIncomplete", data: { firstRemainingLink: liftRemoteControlLinkId(value.inner.firstRemainingLink), retiredLinks: liftRemoteControlLinkCount(value.inner.retiredLinks) } };
    case N.RemoteControlCloseRemoteControlPairingFailure_Tags.EndpointNotRegistered: return { tag: "EndpointNotRegistered" };
    case N.RemoteControlCloseRemoteControlPairingFailure_Tags.IdentityNotHeld: return { tag: "IdentityNotHeld" };
    default: return unexpected(value);
  }
}
export function liftRemoteControlLinkCount(value: N.RemoteControlLinkCount): C.RemoteControlLinkCount {
  return { value: value.value };
}
export function liftRemoteControlCloseRemoteControlPairingControlError(value: N.RemoteControlCloseRemoteControlPairingControlError): C.RemoteControlCloseRemoteControlPairingControlError {
  switch (value.tag) {
    case N.RemoteControlCloseRemoteControlPairingControlError_Tags.NodeStopped: return { tag: "NodeStopped" };
    case N.RemoteControlCloseRemoteControlPairingControlError_Tags.Busy: return { tag: "Busy" };
    case N.RemoteControlCloseRemoteControlPairingControlError_Tags.Failed: return { tag: "Failed", data: { value: liftRemoteControlCloseRemoteControlPairingFailure(value.inner.value) } };
    default: return unexpected(value);
  }
}
export function liftRemoteControlCloseRemoteControlPairingSettlement(value: N.RemoteControlCloseRemoteControlPairingSettlement): C.RemoteControlCloseRemoteControlPairingSettlement {
  switch (value.tag) {
    case N.RemoteControlCloseRemoteControlPairingSettlement_Tags.Completed: return { tag: "Completed", data: { value: liftRemoteControlCloseRemoteControlPairingOutcome(value.inner.value) } };
    case N.RemoteControlCloseRemoteControlPairingSettlement_Tags.Failed: return { tag: "Failed", data: { failure: liftRemoteControlCloseRemoteControlPairingControlError(value.inner.failure) } };
    default: return unexpected(value);
  }
}
export function bindRemoteControlOperations(host: () => N.HostClientHandle) {
  return {
    async beginRemoteControlControllerPairing(begin: C.RemoteControlBeginRemoteControlControllerPairing, options?: {signal: AbortSignal}): Promise<C.RemoteControlBeginRemoteControlControllerPairingSettlement> {
      return liftRemoteControlBeginRemoteControlControllerPairingSettlement(await host().beginRemoteControlControllerPairing(lowerRemoteControlBeginRemoteControlControllerPairing(begin), options));
    },
    async approveRemoteControlControllerPairing(approve: C.RemoteControlApproveRemoteControlControllerPairing, options?: {signal: AbortSignal}): Promise<C.RemoteControlApproveRemoteControlControllerPairingSettlement> {
      return liftRemoteControlApproveRemoteControlControllerPairingSettlement(await host().approveRemoteControlControllerPairing(lowerRemoteControlApproveRemoteControlControllerPairing(approve), options));
    },
    async rejectRemoteControlControllerPairing(reject: C.RemoteControlRejectRemoteControlControllerPairing, options?: {signal: AbortSignal}): Promise<C.RemoteControlRejectRemoteControlControllerPairingSettlement> {
      return liftRemoteControlRejectRemoteControlControllerPairingSettlement(await host().rejectRemoteControlControllerPairing(lowerRemoteControlRejectRemoteControlControllerPairing(reject), options));
    },
    async approveRemoteControlTargetPairing(approve: C.RemoteControlApproveRemoteControlTargetPairing, options?: {signal: AbortSignal}): Promise<C.RemoteControlApproveRemoteControlTargetPairingSettlement> {
      return liftRemoteControlApproveRemoteControlTargetPairingSettlement(await host().approveRemoteControlTargetPairing(lowerRemoteControlApproveRemoteControlTargetPairing(approve), options));
    },
    async rejectRemoteControlTargetPairing(reject: C.RemoteControlRejectRemoteControlTargetPairing, options?: {signal: AbortSignal}): Promise<C.RemoteControlRejectRemoteControlTargetPairingSettlement> {
      return liftRemoteControlRejectRemoteControlTargetPairingSettlement(await host().rejectRemoteControlTargetPairing(lowerRemoteControlRejectRemoteControlTargetPairing(reject), options));
    },
    async remoteControlTargetInventory(options?: {signal: AbortSignal}): Promise<C.RemoteControlTargetInventorySettlement> {
      return liftRemoteControlTargetInventorySettlement(await host().remoteControlTargetInventory(options));
    },
    async resolveRemoteControlTarget(target: C.RemoteControlIdentityHash, options?: {signal: AbortSignal}): Promise<C.RemoteControlResolveRemoteControlTargetSettlement> {
      return liftRemoteControlResolveRemoteControlTargetSettlement(await host().resolveRemoteControlTarget(lowerRemoteControlIdentityHash(target), options));
    },
    async setRemoteControlTargetAccess(access: C.RemoteControlTargetAccess, options?: {signal: AbortSignal}): Promise<C.RemoteControlSetRemoteControlTargetAccessSettlement> {
      return liftRemoteControlSetRemoteControlTargetAccessSettlement(await host().setRemoteControlTargetAccess(lowerRemoteControlTargetAccess(access), options));
    },
    async forgetRemoteControlTarget(target: C.RemoteControlIdentityHash, options?: {signal: AbortSignal}): Promise<C.RemoteControlForgetRemoteControlTargetSettlement> {
      return liftRemoteControlForgetRemoteControlTargetSettlement(await host().forgetRemoteControlTarget(lowerRemoteControlIdentityHash(target), options));
    },
    async openRemoteControlPairing(open: C.RemoteControlOpenRemoteControlPairing, options?: {signal: AbortSignal}): Promise<C.RemoteControlOpenRemoteControlPairingSettlement> {
      return liftRemoteControlOpenRemoteControlPairingSettlement(await host().openRemoteControlPairing(lowerRemoteControlOpenRemoteControlPairing(open), options));
    },
    async closeRemoteControlPairing(options?: {signal: AbortSignal}): Promise<C.RemoteControlCloseRemoteControlPairingSettlement> {
      return liftRemoteControlCloseRemoteControlPairingSettlement(await host().closeRemoteControlPairing(options));
    },
  };
}
export type RemoteControlOperations = ReturnType<typeof bindRemoteControlOperations>;
