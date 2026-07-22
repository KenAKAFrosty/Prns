import json
import shutil
import tempfile
from pathlib import Path

import RNS
import RNS.Discovery


config_home = Path(tempfile.mkdtemp(prefix="prns-stock-interface-skip-"))
interface_types = (
    "PrnsUsbAuto",
    "PrnsUsbAutoInterface",
    "PrnsBluetoothAuto",
    "PrnsBluetoothAutoInterface",
    "PrnsBleAuto",
    "PrnsBleAutoInterface",
    "PrnsWebSocketClient",
    "PrnsWebSocketClientInterface",
    "PrnsWebSocketServer",
    "PrnsWebSocketServerInterface",
)
try:
    stanzas = "\n".join(
        f"[[{interface_type} stanza]]\ntype = {interface_type}\nenabled = Yes"
        for interface_type in interface_types
    )
    (config_home / "config").write_text(
        f"[reticulum]\nshare_instance = No\n\n[interfaces]\n{stanzas}\n",
        encoding="utf-8",
    )
    RNS.Reticulum(configdir=str(config_home), loglevel=RNS.LOG_VERBOSE)
    registered = [interface.name for interface in RNS.Transport.interfaces]
    print(
        "PRNS_STOCK_SKIP_RESULT="
        + json.dumps(
            {
                "version": RNS.__version__,
                "discovery_default_stamp_cost": RNS.Discovery.InterfaceAnnouncer.DEFAULT_STAMP_VALUE,
                "registered": registered,
                "configured": list(interface_types),
            }
        ),
        flush=True,
    )
finally:
    RNS.Reticulum.exit_handler()
    shutil.rmtree(config_home, ignore_errors=True)
