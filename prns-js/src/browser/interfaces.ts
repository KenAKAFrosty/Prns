import { AutoWifiInterface } from "./auto_wifi/index.js";
import { BluetoothInterface } from "./bluetooth/index.js";
import { RNodeInterface } from "./rnode.js";
import type { RuntimeHost } from "./runtime.js";
import { UsbAutoInterface } from "./usb_auto/index.js";
import { WebSocketInterface } from "./websocket/index.js";

export class PrnsInterfaces {
  readonly usbAuto: UsbAutoInterface;
  readonly rnode: RNodeInterface;
  readonly bluetooth: BluetoothInterface;
  readonly autoWifi: AutoWifiInterface;
  readonly webSocket: WebSocketInterface;

  constructor(host: RuntimeHost, autoWifiSelectionSeed?: Uint8Array) {
    this.usbAuto = new UsbAutoInterface(host);
    this.rnode = new RNodeInterface(host);
    this.bluetooth = new BluetoothInterface(host);
    this.autoWifi = new AutoWifiInterface(host, autoWifiSelectionSeed);
    this.webSocket = new WebSocketInterface(host);
  }
}
