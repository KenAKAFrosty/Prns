from __future__ import annotations

from validation.hardening.embedded_isa.contract import Architecture, Compiler
from validation.hardening.embedded_readiness.model import (
    CheckState,
    CommandOutput,
    ReadinessCheck,
    ReadinessContract,
    ReadinessLane,
)
from validation.hardening.embedded_readiness.system import SystemProbe


def inspect(contract: ReadinessContract, probe: SystemProbe) -> tuple[ReadinessCheck, ...]:
    rust_targets = tuple(
        architecture.rust_target
        for architecture in contract.architectures
        if architecture.compiler is Compiler.UPSTREAM
    )
    checks = [
        inspect_rust(
            probe,
            ReadinessLane.ISA,
            "target-ISA Rust",
            contract.isa_toolchain,
            contract.isa_toolchain,
            rust_targets,
            (),
        ),
        inspect_rust(
            probe,
            ReadinessLane.RESOURCES,
            "resource Rust",
            "stable",
            None,
            rust_targets,
            ("llvm-tools-preview",),
        ),
        inspect_miri(probe, contract),
        inspect_esp(probe, contract),
    ]
    checks.extend(
        inspect_emulator(probe, architecture)
        for architecture in contract.architectures
    )
    return tuple(checks)


def inspect_rust(
    probe: SystemProbe,
    lane: ReadinessLane,
    subject: str,
    toolchain: str,
    exact_version: str | None,
    targets: tuple[str, ...],
    components: tuple[str, ...],
) -> ReadinessCheck:
    identity = probe.run(("rustup", "run", toolchain, "rustc", "--version"))
    setup = (
        f"rustup toolchain install {toolchain} --profile minimal",
        rustup_add("target", toolchain, targets),
        rustup_add("component", toolchain, components),
    )
    setup = tuple(command for command in setup if command)
    if identity.returncode != 0:
        return ReadinessCheck(lane, subject, CheckState.MISSING, first_line(identity), setup)
    banner = first_line(identity)
    if exact_version is not None and not banner.startswith(f"rustc {exact_version} "):
        return ReadinessCheck(
            lane,
            subject,
            CheckState.MISMATCH,
            f"found {banner!r}; expected Rust {exact_version}",
            setup,
        )
    installed_targets = installed(probe, toolchain, "target")
    installed_components = installed(probe, toolchain, "component")
    missing_targets = tuple(target for target in targets if target not in installed_targets)
    missing_components = tuple(
        component
        for component in components
        if not component_installed(component, installed_components)
    )
    if missing_targets or missing_components:
        detail = []
        if missing_targets:
            detail.append(f"targets={','.join(missing_targets)}")
        if missing_components:
            detail.append(f"components={','.join(missing_components)}")
        return ReadinessCheck(
            lane,
            subject,
            CheckState.MISSING,
            "missing " + " ".join(detail),
            setup,
        )
    return ReadinessCheck(lane, subject, CheckState.READY, banner)


def installed(probe: SystemProbe, toolchain: str, kind: str) -> frozenset[str]:
    result = probe.run(("rustup", kind, "list", "--toolchain", toolchain, "--installed"))
    if result.returncode != 0:
        return frozenset()
    return frozenset(result.lines())


def component_installed(component: str, installed_components: frozenset[str]) -> bool:
    prefix = "llvm-tools" if component == "llvm-tools-preview" else component
    return any(
        value == prefix or value.startswith(f"{prefix}-")
        for value in installed_components
    )


def rustup_add(kind: str, toolchain: str, values: tuple[str, ...]) -> str:
    if not values:
        return ""
    return f"rustup {kind} add --toolchain {toolchain} {' '.join(values)}"


def inspect_miri(probe: SystemProbe, contract: ReadinessContract) -> ReadinessCheck:
    toolchain = contract.miri_toolchain
    setup = (
        f"rustup toolchain install {toolchain} --profile minimal",
        f"rustup component add --toolchain {toolchain} miri rust-src",
        f"cargo +{toolchain} miri setup",
    )
    identity = probe.run(("rustup", "run", toolchain, "rustc", "--version"))
    if identity.returncode != 0:
        return ReadinessCheck(
            ReadinessLane.MIRI,
            "embedded Miri",
            CheckState.MISSING,
            first_line(identity),
            setup,
        )
    components = installed(probe, toolchain, "component")
    missing = tuple(
        component
        for component in ("miri", "rust-src")
        if not component_installed(component, components)
    )
    miri = probe.run(("rustup", "run", toolchain, "cargo", "miri", "--version"))
    if missing or miri.returncode != 0:
        detail = f"missing components={','.join(missing)}" if missing else first_line(miri)
        return ReadinessCheck(
            ReadinessLane.MIRI,
            "embedded Miri",
            CheckState.MISSING,
            detail,
            setup,
        )
    return ReadinessCheck(
        ReadinessLane.MIRI,
        "embedded Miri",
        CheckState.READY,
        f"{toolchain}; scenarios={contract.miri_scenarios}; {first_line(miri)}",
    )


def inspect_esp(probe: SystemProbe, contract: ReadinessContract) -> ReadinessCheck:
    identity = contract.esp_identity
    setup = (
        f"cargo install espup --version {identity.espup_version} --locked",
        "espup install --std --targets esp32s3 "
        f"--toolchain-version {identity.rust_toolchain_version} "
        f"--crosstool-toolchain-version {identity.crosstool_version}",
    )
    gaps = []
    mismatches = []
    espup = probe.find("espup")
    if espup is None:
        gaps.append("espup")
    else:
        output = probe.run((str(espup), "--version"))
        expected = f"espup {identity.espup_version}"
        if first_line(output) != expected:
            mismatches.append(f"espup={first_line(output)!r}")
    rustc = probe.run(("rustup", "run", "esp", "rustc", "-vV"))
    if rustc.returncode != 0:
        gaps.append("ESP Rust")
    elif first_line(rustc) != identity.rustc_banner:
        mismatches.append(f"rustc={first_line(rustc)!r}")
    gcc = probe.find("xtensa-esp32s3-elf-gcc", contract.esp_environment.search_paths)
    if gcc is None:
        gaps.append("xtensa GCC")
    else:
        output = probe.run((str(gcc), "--version"))
        if first_line(output) != identity.gcc_banner:
            mismatches.append(f"gcc={first_line(output)!r}")
    objdump = probe.find(
        "xtensa-esp32s3-elf-objdump", contract.esp_environment.search_paths
    )
    if objdump is None:
        gaps.append("xtensa objdump")
    libclang = contract.esp_environment.libclang_path
    if libclang is None or not libclang.is_dir():
        gaps.append("LIBCLANG_PATH")
    if mismatches or gaps:
        detail = list(mismatches)
        if gaps:
            detail.append("missing " + ", ".join(gaps))
        return ReadinessCheck(
            ReadinessLane.RESOURCES,
            "ESP resource toolchain",
            CheckState.MISMATCH if mismatches else CheckState.MISSING,
            "; ".join(detail),
            setup,
        )
    return ReadinessCheck(
        ReadinessLane.RESOURCES,
        "ESP resource toolchain",
        CheckState.READY,
        f"espup {identity.espup_version}; ESP Rust {identity.rust_toolchain_version}; "
        f"crosstool-NG {identity.crosstool_version}",
    )


def inspect_emulator(
    probe: SystemProbe, architecture: Architecture
) -> ReadinessCheck:
    setup = (
        f"install QEMU {architecture.emulator.version} from "
        f"{architecture.emulator.source_url}",
        f"verify source SHA-256 {architecture.emulator.source_sha256}",
        f"expose {architecture.emulator.executable} on PATH",
    )
    executable = probe.find(architecture.emulator.executable)
    if executable is None:
        return ReadinessCheck(
            ReadinessLane.ISA,
            f"{architecture.identifier} emulator",
            CheckState.MISSING,
            f"{architecture.emulator.executable} was not found",
            setup,
        )
    output = probe.run((str(executable), "--version"))
    expected = f"QEMU emulator version {architecture.emulator.version}"
    found = first_line(output)
    if output.returncode != 0 or found != expected:
        return ReadinessCheck(
            ReadinessLane.ISA,
            f"{architecture.identifier} emulator",
            CheckState.MISMATCH,
            f"found {found!r}; expected {expected!r}",
            setup,
        )
    return ReadinessCheck(
        ReadinessLane.ISA,
        f"{architecture.identifier} emulator",
        CheckState.READY,
        f"{executable}; {expected}",
    )


def first_line(output: CommandOutput) -> str:
    lines = output.lines()
    return lines[0] if lines else "command produced no identity"
