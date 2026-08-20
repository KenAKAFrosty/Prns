import json
import shutil
import tempfile
from pathlib import Path

import RNS


config_home = Path(tempfile.mkdtemp(prefix="prns-stock-section-skip-"))
source = (
    "[reticulum]\n"
    "share_instance = No\n"
    "\n"
    "[prns]\n"
    "resource_mem_in = 64 MiB\n"
    "resource_mem_out = 0\n"
    "\n"
    "[interfaces]\n"
)
config_path = config_home / "config"
try:
    config_path.write_text(source, encoding="utf-8")
    reticulum = RNS.Reticulum(configdir=str(config_home), loglevel=RNS.LOG_VERBOSE)
    print(
        "PRNS_STOCK_SECTION_RESULT="
        + json.dumps(
            {
                "version": RNS.__version__,
                "loaded_prns": dict(reticulum.config["prns"]),
                "config_unchanged": config_path.read_text(encoding="utf-8") == source,
                "registered": [interface.name for interface in RNS.Transport.interfaces],
            }
        ),
        flush=True,
    )
finally:
    RNS.Reticulum.exit_handler()
    shutil.rmtree(config_home, ignore_errors=True)
