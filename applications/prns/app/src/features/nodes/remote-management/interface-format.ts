import * as Bindings from "@prns-internal/expo";

export function connectionLabel(connection: Bindings.RemoteConnectionState): string {
  switch (connection) {
    case Bindings.RemoteConnectionState.Initializing:
      return "Starting";
    case Bindings.RemoteConnectionState.Connected:
      return "Connected";
    case Bindings.RemoteConnectionState.Degraded:
      return "Limited connection";
    case Bindings.RemoteConnectionState.Reconnecting:
      return "Reconnecting";
    case Bindings.RemoteConnectionState.Failed:
      return "Connection failed";
    case Bindings.RemoteConnectionState.Disconnected:
      return "Disconnected";
    case Bindings.RemoteConnectionState.Disabled:
      return "Off";
    case Bindings.RemoteConnectionState.Unknown:
      return "Not reported";
  }
}

export const interfaceModes = [
  { value: Bindings.RemoteInterfaceMode.Full, label: "Full" },
  { value: Bindings.RemoteInterfaceMode.PointToPoint, label: "Point-to-point" },
  { value: Bindings.RemoteInterfaceMode.AccessPoint, label: "Access point" },
  { value: Bindings.RemoteInterfaceMode.Roaming, label: "Roaming" },
  { value: Bindings.RemoteInterfaceMode.Boundary, label: "Boundary" },
  { value: Bindings.RemoteInterfaceMode.Gateway, label: "Gateway" },
  { value: Bindings.RemoteInterfaceMode.Internal, label: "Internal" },
] as const;

export const loraRegions = [
  { value: Bindings.RemoteLoRaRegion.Us915, label: "US 915" },
  { value: Bindings.RemoteLoRaRegion.Au915, label: "AU 915" },
  { value: Bindings.RemoteLoRaRegion.Eu433, label: "EU 433" },
  { value: Bindings.RemoteLoRaRegion.Eu865, label: "EU 865" },
  { value: Bindings.RemoteLoRaRegion.Eu868, label: "EU 868" },
  { value: Bindings.RemoteLoRaRegion.Eu869, label: "EU 869" },
  { value: Bindings.RemoteLoRaRegion.As923, label: "AS 923" },
  { value: Bindings.RemoteLoRaRegion.In865, label: "IN 865" },
  { value: Bindings.RemoteLoRaRegion.Cn470, label: "CN 470" },
  { value: Bindings.RemoteLoRaRegion.Kr920, label: "KR 920" },
  { value: Bindings.RemoteLoRaRegion.Jp920, label: "JP 920" },
  { value: Bindings.RemoteLoRaRegion.Custom, label: "Custom" },
] as const;

export function interfaceKindLabel(kind: string): string {
  const common: Readonly<Record<string, string>> = {
    "auto-wifi": "Wi-Fi",
    "wifi-peer": "Wi-Fi peer",
    "bluetooth-auto": "Bluetooth",
    "bluetooth-peer": "Bluetooth peer",
    lora: "LoRa",
    "usb-auto-host": "USB",
    "usb-auto-device": "USB",
    "esp-now": "ESP-NOW",
    "tcp-client": "TCP client",
    "tcp-server": "TCP server",
    "tcp-server-peer": "TCP peer",
    udp: "UDP",
    serial: "Serial",
    "local-server": "Shared connection",
    "local-client": "Shared connection client",
  };
  return common[kind] ?? kind.replaceAll("-", " ");
}

export function radioSummary(radio: Bindings.RemotePeerRadio): string | null {
  switch (radio.tag) {
    case Bindings.RemotePeerRadio_Tags.NotRadio:
      return null;
    case Bindings.RemotePeerRadio_Tags.Pending:
      return "Waiting for signal information";
    case Bindings.RemotePeerRadio_Tags.Unavailable:
      return "Signal information unavailable";
    case Bindings.RemotePeerRadio_Tags.Measured: {
      const readings = [`${radio.inner.rssiDbm} dBm`];
      if (radio.inner.snrQuarterDb !== undefined)
        readings.push(`${radio.inner.snrQuarterDb / 4} dB signal-to-noise`);
      if (radio.inner.qualityTenthsPercent !== undefined)
        readings.push(`${radio.inner.qualityTenthsPercent / 10}% quality`);
      return readings.join(" · ");
    }
  }
}
