import * as Bindings from "@prns-internal/expo";

import { BodyText, Card, KeyValue, Subheading } from "@/ui/primitives";

export function RemoteChangeStatusCard({
  operation,
}: {
  readonly operation: Bindings.RemoteChangeOperation;
}) {
  return (
    <Card>
      <Subheading>Last change</Subheading>
      <KeyValue label="Setting" value={remoteChangeLabel(operation.change)} />
      <BodyText>{remoteChangeStatusMessage(operation.status)}</BodyText>
    </Card>
  );
}

function remoteChangeLabel(change: Bindings.RemoteNodeChange): string {
  switch (change.tag) {
    case Bindings.RemoteNodeChange_Tags.InterfacePower:
      return change.inner.enabled ? "Turn interface on" : "Turn interface off";
    case Bindings.RemoteNodeChange_Tags.InterfaceMode:
      return "Interface mode";
    case Bindings.RemoteNodeChange_Tags.InterfaceGroup:
      return "Interface group";
    case Bindings.RemoteNodeChange_Tags.InterfaceLoRa:
      return "LoRa settings";
    case Bindings.RemoteNodeChange_Tags.DiscoveryGroups:
      return "Discovery groups";
    case Bindings.RemoteNodeChange_Tags.GnssPower:
      return change.inner.enabled ? "Turn positioning on" : "Turn positioning off";
    case Bindings.RemoteNodeChange_Tags.DisplayVisibility:
      return change.inner.visible ? "Show display" : "Hide display";
    case Bindings.RemoteNodeChange_Tags.DisplayAutoOff:
      return change.inner.enabled ? "Allow display to turn off" : "Keep display on";
    case Bindings.RemoteNodeChange_Tags.SystemPower:
      return change.inner.awake ? "Wake node" : "Sleep node";
    case Bindings.RemoteNodeChange_Tags.StationUplink:
      return change.inner.enabled ? "Enable Wi-Fi connection" : "Disable Wi-Fi connection";
    case Bindings.RemoteNodeChange_Tags.RadioMode:
      return change.inner.mode === Bindings.RemoteRadioMode.Bluetooth
        ? "Use Bluetooth"
        : "Use Wi-Fi hotspot";
    case Bindings.RemoteNodeChange_Tags.SleepRadios:
      return "Pause radios";
    case Bindings.RemoteNodeChange_Tags.WakeRadios:
      return "Resume radios";
  }
}

export function remoteChangeStatusMessage(status: Bindings.RemoteChangeStatus): string {
  switch (status.tag) {
    case Bindings.RemoteChangeStatus_Tags.Pending:
      return "Waiting for the node to confirm the change…";
    case Bindings.RemoteChangeStatus_Tags.Applied:
      return "The node applied the change. Refresh its details to see the latest information.";
    case Bindings.RemoteChangeStatus_Tags.Unchanged:
      return "The node already had this setting. Nothing changed.";
    case Bindings.RemoteChangeStatus_Tags.Scheduled:
      return "The node accepted the change and will apply it shortly. It may disconnect; reconnect and refresh to check the result.";
    case Bindings.RemoteChangeStatus_Tags.OutcomeUnknown:
      return "The result could not be confirmed. The node may have applied the change. This request was not repeated. Reconnect and check before trying again.";
    case Bindings.RemoteChangeStatus_Tags.Failed:
      return remoteManagementFailureMessage(status.inner.stage);
  }
}

export function remoteManagementFailureMessage(
  stage: Bindings.RemoteManagementFailureStage,
): string {
  switch (stage) {
    case Bindings.RemoteManagementFailureStage.Input:
      return "Check the values you entered. They are not valid for this setting.";
    case Bindings.RemoteManagementFailureStage.Node:
      return "This device's node is not running. Start it, then try again.";
    case Bindings.RemoteManagementFailureStage.Inventory:
      return "This paired node is no longer available.";
    case Bindings.RemoteManagementFailureStage.Route:
    case Bindings.RemoteManagementFailureStage.Link:
      return "The node could not be reached. Keep it on and nearby, then try again.";
    case Bindings.RemoteManagementFailureStage.Identification:
      return "The saved pairing could not be used. Check that this node is still paired.";
    case Bindings.RemoteManagementFailureStage.Permission:
      return "This pairing does not allow that action.";
    case Bindings.RemoteManagementFailureStage.Unsupported:
      return "This node does not support that action.";
    case Bindings.RemoteManagementFailureStage.UnknownInterface:
      return "This interface is no longer available. Refresh the node's interfaces.";
    case Bindings.RemoteManagementFailureStage.Request:
      return "The node could not complete the request.";
    case Bindings.RemoteManagementFailureStage.Timeout:
      return "The node did not answer in time. Check its connection before trying again.";
    case Bindings.RemoteManagementFailureStage.Busy:
      return "Another operation is in progress. Wait for it to finish, then try again.";
    case Bindings.RemoteManagementFailureStage.Response:
      return "The node's response could not be understood. Refresh its details before trying again.";
    case Bindings.RemoteManagementFailureStage.Persistence:
      return "The node could not save the change. Refresh its settings before trying again.";
    case Bindings.RemoteManagementFailureStage.Rollback:
      return "The node could not restore its previous settings. Check the node before making another change.";
  }
}
