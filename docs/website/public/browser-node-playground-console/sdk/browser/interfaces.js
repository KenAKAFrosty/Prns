import { AutoWifiInterface } from "./auto_wifi/index.js";
import { BluetoothInterface } from "./bluetooth/index.js";
import { RNodeInterface } from "./rnode.js";
import { UsbAutoInterface } from "./usb_auto/index.js";
import { WebSocketInterface } from "./websocket/index.js";
export class PrnsInterfaces {
    usbAuto;
    rnode;
    bluetooth;
    autoWifi;
    webSocket;
    constructor(host) {
        this.usbAuto = new UsbAutoInterface(host);
        this.rnode = new RNodeInterface(host);
        this.bluetooth = new BluetoothInterface(host);
        this.autoWifi = new AutoWifiInterface(host);
        this.webSocket = new WebSocketInterface(host);
    }
}
