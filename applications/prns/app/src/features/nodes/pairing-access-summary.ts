import { RemoteControlRequestKind } from "@prns-internal/expo";

type AccessCategory = "read" | "change" | "share" | "manageAccess";

const categoryLabels = {
  read: "View node information",
  change: "Change node settings",
  share: "Share the node address",
  manageAccess: "Manage other devices’ access",
} as const satisfies Record<AccessCategory, string>;

const categoryOrder = ["read", "change", "share", "manageAccess"] as const;

/** A compact overview only; the confirmation also exposes each exact permission. */
export function summarizePairingAccess(permissions: readonly RemoteControlRequestKind[]): string {
  const categories = new Set(permissions.map(accessCategory));
  if (categories.size === 0) {
    return "No node controls requested.";
  }
  return `${categoryOrder
    .filter((category) => categories.has(category))
    .map((category) => categoryLabels[category])
    .join(". ")}.`;
}

function accessCategory(kind: RemoteControlRequestKind): AccessCategory {
  switch (kind) {
    case RemoteControlRequestKind.Describe:
    case RemoteControlRequestKind.InventoryInterfaces:
    case RemoteControlRequestKind.InventoryInterfacePeers:
    case RemoteControlRequestKind.InventoryInterfaceConfig:
    case RemoteControlRequestKind.DescribeBuild:
    case RemoteControlRequestKind.InventoryControllers:
    case RemoteControlRequestKind.DescribePower:
    case RemoteControlRequestKind.InspectWifiTransaction:
    case RemoteControlRequestKind.InventoryInterfaceDiscoveryGroups:
      return "read";
    case RemoteControlRequestKind.SetInterfacePower:
    case RemoteControlRequestKind.SleepRadios:
    case RemoteControlRequestKind.WakeRadios:
    case RemoteControlRequestKind.SetInterfaceMode:
    case RemoteControlRequestKind.SetInterfaceGroup:
    case RemoteControlRequestKind.SetInterfaceLoRaProfile:
    case RemoteControlRequestKind.SetInterfaceWifiStation:
    case RemoteControlRequestKind.SetSystemPower:
    case RemoteControlRequestKind.SetGnssPower:
    case RemoteControlRequestKind.SetDisplayVisibility:
    case RemoteControlRequestKind.SetDisplayAutoOff:
    case RemoteControlRequestKind.SetStationUplink:
    case RemoteControlRequestKind.SetEspRadioMode:
    case RemoteControlRequestKind.StageWifiCredentials:
    case RemoteControlRequestKind.ActivateWifiCredentials:
    case RemoteControlRequestKind.ConfirmWifiCredentials:
    case RemoteControlRequestKind.CancelWifiCredentials:
    case RemoteControlRequestKind.ReplaceInterfaceDiscoveryGroups:
      return "change";
    case RemoteControlRequestKind.AnnounceSelf:
      return "share";
    case RemoteControlRequestKind.AuthorizeController:
    case RemoteControlRequestKind.RevokeController:
      return "manageAccess";
  }
  const unhandled: never = kind;
  throw new Error(`Unhandled pairing permission: ${unhandled}`);
}
