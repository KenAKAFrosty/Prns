import json
import sys

import RNS
from RNS.Interfaces.Interface import Interface
from RNS.Interfaces.RNodeInterface import RNodeInterface


class OpenWire:
    is_open = True


def fake_open(interface):
    interface.serial = OpenWire()


Interface.__init__ = lambda _interface: None
RNodeInterface.open_port = fake_open
RNodeInterface.configure_device = lambda _interface: None

base = {
    "name": "oracle",
    "frequency": "868000000",
    "bandwidth": "125000",
    "txpower": "7",
    "spreadingfactor": "8",
    "codingrate": "5",
}

results = []
for port in json.load(sys.stdin):
    interface = RNodeInterface(object(), dict(base, port=port))
    results.append(
        {
            "serial_port": interface.port,
            "use_ble": interface.use_ble,
            "ble_name": interface.ble_name,
            "ble_addr": interface.ble_addr,
            "use_tcp": interface.use_tcp,
            "tcp_host": interface.tcp_host,
        }
    )

json.dump({"version": RNS.__version__, "results": results}, sys.stdout)
