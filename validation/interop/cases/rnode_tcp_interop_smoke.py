from pathlib import Path

from validation.interop.harness import (
    InteropCase,
    PeerSpec,
    cargo_binary,
    case_main,
    environment,
    reference_python,
    run_checked,
)


ROOT = Path(__file__).resolve().parents[3]
PRNSD_MANIFEST = ROOT / "prnsd/Cargo.toml"
STOCK_DEVICE = ROOT / "validation/interop/peers/rns_rnode_tcp_device.py"
SUCCESS = "PASS: Prnsd rejected hostile RNode bring-up sequences and recovered against the stock RNS split-frame oracle"


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with InteropCase() as case:
        config = case.work / "prns"
        ready = case.work / "device-ready"
        run_checked(
            (str(python), str(STOCK_DEVICE), "prepare", str(config)),
            "stock RNS RNode TCP configuration was not prepared",
        )
        device = case.start(
            PeerSpec(
                "stock RNS RNode TCP device",
                (str(python), str(STOCK_DEVICE), "serve", str(ready)),
                environment({}),
            )
        )
        case.wait_for_path(device, ready, 10)
        daemon = case.start(
            PeerSpec(
                "Prnsd RNode TCP client",
                (str(prnsd), "run", "--log-format", "json", "--config", str(config)),
                environment({"RUST_LOG": "info"}),
            )
        )
        case.wait_for_all(
            [
                (device, "RNODE_TCP_DEVICE_OK"),
                (daemon, '"event":"daemon_ready'),
            ],
            60,
        )
        case.wait_for_exit(device, 5)


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
