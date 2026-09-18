import {
  DevelopmentNodeRuntime,
  RemoteControlControllerAuthority,
  RemoteControlRequestKind,
} from "@prns-internal/expo";
export function formatBytes(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function formatRequestKind(kind: RemoteControlRequestKind): string {
  switch (kind) {
    case RemoteControlRequestKind.AnnounceSelf:
      return "Share node address";
    case RemoteControlRequestKind.Describe:
      return "View node information";
    case RemoteControlRequestKind.InventoryInterfaces:
      return "View interfaces";
    case RemoteControlRequestKind.SetInterfacePower:
      return "Turn interfaces on or off";
    case RemoteControlRequestKind.SleepRadios:
      return "Pause radios";
    case RemoteControlRequestKind.WakeRadios:
      return "Resume radios";
    case RemoteControlRequestKind.SetInterfaceMode:
      return "Change interface mode";
    case RemoteControlRequestKind.SetInterfaceGroup:
      return "Change interface group";
    case RemoteControlRequestKind.InventoryInterfacePeers:
      return "View connected peers";
    case RemoteControlRequestKind.InventoryInterfaceConfig:
      return "View interface settings";
    case RemoteControlRequestKind.SetInterfaceLoRaProfile:
      return "Change LoRa settings";
    case RemoteControlRequestKind.DescribeBuild:
      return "View firmware version";
    case RemoteControlRequestKind.SetInterfaceWifiStation:
      return "Change Wi-Fi network";
    case RemoteControlRequestKind.InventoryControllers:
      return "View authorized controllers";
    case RemoteControlRequestKind.AuthorizeController:
      return "Grant controller access";
    case RemoteControlRequestKind.RevokeController:
      return "Revoke controller access";
    case RemoteControlRequestKind.DescribePower:
      return "View battery and power";
    case RemoteControlRequestKind.SetSystemPower:
      return "Sleep or wake the node";
    case RemoteControlRequestKind.SetGnssPower:
      return "Control satellite positioning";
    case RemoteControlRequestKind.SetDisplayVisibility:
      return "Show or hide the display";
    case RemoteControlRequestKind.SetDisplayAutoOff:
      return "Change automatic display-off";
    case RemoteControlRequestKind.SetStationUplink:
      return "Enable or disable the Wi-Fi connection";
    case RemoteControlRequestKind.SetEspRadioMode:
      return "Change Bluetooth or hotspot mode";
    case RemoteControlRequestKind.StageWifiCredentials:
      return "Prepare a Wi-Fi network change";
    case RemoteControlRequestKind.ActivateWifiCredentials:
      return "Try a Wi-Fi network";
    case RemoteControlRequestKind.ConfirmWifiCredentials:
      return "Keep the new Wi-Fi network";
    case RemoteControlRequestKind.CancelWifiCredentials:
      return "Restore the previous Wi-Fi network";
    case RemoteControlRequestKind.InspectWifiTransaction:
      return "Check Wi-Fi setup";
  }
}

export function formatControllerAuthority(authority: RemoteControlControllerAuthority): string {
  switch (authority) {
    case RemoteControlControllerAuthority.Operator:
      return "Operator";
    case RemoteControlControllerAuthority.Administrator:
      return "Administrator";
  }
}

export function formatRuntime(runtime: DevelopmentNodeRuntime) {
  switch (runtime) {
    case DevelopmentNodeRuntime.Failed:
      return "Failed";
    case DevelopmentNodeRuntime.Running:
      return "Running";
    case DevelopmentNodeRuntime.Starting:
      return "Starting";
    case DevelopmentNodeRuntime.Stopped:
      return "Stopped";
    case DevelopmentNodeRuntime.Stopping:
      return "Stopping";
  }
}
