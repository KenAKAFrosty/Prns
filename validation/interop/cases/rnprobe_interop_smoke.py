import re
from pathlib import Path

from validation.interop.harness import (
    FailureKind,
    InteropCase,
    InteropFailure,
    PeerSpec,
    PortLease,
    cargo_binary,
    case_main,
    environment,
    reference_python,
    require_evidence,
    require_output_marker,
    run_checked,
    run_expect_status,
)


ROOT = Path(__file__).resolve().parents[3]
PRNSD_MANIFEST = ROOT / "prnsd/Cargo.toml"
STOCK_SERVER = ROOT / "validation/interop/peers/rns_rnprobe_server.py"
READY_PATTERN = re.compile(
    r"^RNPROBE_SERVER_READY ([0-9a-f]{32}) ([0-9a-f]{32})$",
    re.MULTILINE,
)
SUCCESS = "PASS: Prnsd probe exchanged delivery proofs with stock RNS and preserved loss exit 2"


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with PortLease() as bus_port, PortLease() as control_port, InteropCase() as case:
        config = case.work / "config"
        run_checked(
            (
                str(python),
                str(STOCK_SERVER),
                "prepare",
                str(config),
                str(bus_port.port),
                str(control_port.port),
            ),
            "stock RNS did not prepare the rnprobe configuration",
        )
        bus_port.release()
        control_port.release()
        server = case.start_reference_rns(
            PeerSpec(
                "stock RNS rnprobe server",
                (str(python), str(STOCK_SERVER), "serve", str(config)),
                environment({}),
            )
        )
        case.wait_for(server, "RNPROBE_SERVER_READY ", 10)
        ready = READY_PATTERN.search(case.read_log(server))
        if ready is None:
            raise InteropFailure(
                FailureKind.EVIDENCE_MISSING,
                "stock RNS did not report valid probe destination hashes",
            )
        probe_hash, silent_hash = ready.groups()

        probe_result = run_checked(
            (
                str(prnsd),
                "probe",
                "--config",
                str(config),
                "-s",
                "24",
                "-n",
                "2",
                "-t",
                "5",
                "-w",
                "0.1",
                "-v",
                "rnstransport.probe",
                probe_hash,
            ),
            "Prnsd could not probe the stock RNS responder",
        )
        reply_marker = f"Valid reply from <{probe_hash}>"
        require_evidence(
            probe_result.count(reply_marker) == 2,
            "Prnsd did not settle exactly two stock RNS delivery proofs",
        )
        require_output_marker(
            probe_result,
            "Sent 2, received 2, packet loss 0%",
            "Prnsd did not report lossless stock RNS probes",
        )

        silent_result = run_expect_status(
            (
                str(prnsd),
                "probe",
                "--config",
                str(config),
                "-n",
                "1",
                "-t",
                "0.5",
                "oracle.silent",
                silent_hash,
            ),
            2,
            "Prnsd did not preserve the all-loss rnprobe exit status",
        )
        require_output_marker(
            silent_result,
            "Probe timed out",
            "Prnsd did not report the silent stock RNS probe timeout",
        )
        require_output_marker(
            silent_result,
            "Sent 1, received 0, packet loss 100%",
            "Prnsd did not preserve stock RNS packet-loss semantics",
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
