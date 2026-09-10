#!/usr/bin/env python3
"""Build or verify the two pinned UniFFI runtime npm packages. No install hooks."""

import argparse
import base64
import hashlib
import inspect
import json
import os
from pathlib import Path, PurePosixPath
import plistlib
import re
import shutil
import subprocess
import tarfile


HERE = Path(__file__).resolve().parent
VENDOR = HERE.parents[1] / "vendor" / "ubrn"
LOCK = VENDOR / "source-lock.json"
RECEIPT = VENDOR / "receipt.json"
REQUIRED_NATIVE = (
    "prebuilt/android/arm64-v8a/libuniffi_runtime_jsi.so",
    "prebuilt/android/x86_64/libuniffi_runtime_jsi.so",
    "prebuilt/ios/UniffiRuntimeJsi.xcframework/Info.plist",
    "prebuilt/ios/UniffiRuntimeJsi.xcframework/ios-arm64/UniffiRuntimeJsi.framework/UniffiRuntimeJsi",
    "prebuilt/ios/UniffiRuntimeJsi.xcframework/ios-arm64_x86_64-simulator/UniffiRuntimeJsi.framework/UniffiRuntimeJsi",
    "prebuilt/ios/UniffiRuntimeJsi.xcframework/ios-arm64/UniffiRuntimeJsi.framework/Info.plist",
    "prebuilt/ios/UniffiRuntimeJsi.xcframework/ios-arm64_x86_64-simulator/UniffiRuntimeJsi.framework/Info.plist",
)


def run(*args, cwd=None, env=None, capture=False):
    result = subprocess.run(
        [str(arg) for arg in args], cwd=cwd, env=env, check=True,
        stdout=subprocess.PIPE if capture else None, text=True,
    )
    return result.stdout.strip() if capture else None


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inputs():
    lock = json.loads(LOCK.read_text())
    for patch in lock["patches"]:
        if digest(VENDOR / patch["file"]) != patch["sha256"]:
            raise ValueError(f"patch digest mismatch: {patch['file']}")
    return lock


def prepare(cache, lock):
    # The input digest gives each upstream/patch set its own disposable checkout.
    source = cache / digest(LOCK)[:16] / "source"
    if not source.exists():
        source.mkdir(parents=True)
        run("git", "init", "--quiet", source)
        run("git", "fetch", "--depth=1", lock["upstream"]["repository"],
            lock["upstream"]["revision"], cwd=source)
        run("git", "checkout", "--detach", "FETCH_HEAD", cwd=source)
        for patch in lock["patches"]:
            run("git", "apply", "--index", VENDOR / patch["file"], cwd=source)
        # Local commit identity is immaterial; the receipt records the tree.
        run("git", "-c", "user.name=PRNS vendor builder", "-c",
            "user.email=vendor@invalid", "commit", "--quiet", "-m",
            "Apply the pinned PRNS UniFFI patches", cwd=source)
    if run("git", "rev-parse", "HEAD^", cwd=source, capture=True) != lock["upstream"]["revision"]:
        raise ValueError("cached source is not based on the pinned upstream revision")
    run("git", "diff", "--exit-code", "HEAD", cwd=source)
    # Reapply into an isolated index to verify the expected tree, including on reuse.
    index = source.parent / "expected-index"
    index.unlink(missing_ok=True)
    env = dict(os.environ, GIT_INDEX_FILE=str(index))
    run("git", "read-tree", lock["upstream"]["revision"], cwd=source, env=env)
    for patch in lock["patches"]:
        run("git", "apply", "--cached", VENDOR / patch["file"], cwd=source, env=env)
    expected = run("git", "write-tree", cwd=source, env=env, capture=True)
    actual = run("git", "rev-parse", "HEAD^{tree}", cwd=source, capture=True)
    if actual != expected:
        raise ValueError("cached source tree differs from the pinned patches")
    return source, actual


def build_environment(cache, lock, toolchain):
    expected = lock["toolchain"]
    # Select real binaries as well as the rustup name. Setting RUSTUP_TOOLCHAIN
    # alone does not affect a system Cargo/rustc ahead of the rustup shims.
    rustc = Path(run("rustup", "which", "--toolchain", toolchain, "rustc", capture=True))
    env = dict(os.environ, PATH=str(rustc.parent) + os.pathsep + os.environ["PATH"],
               RUSTUP_TOOLCHAIN=toolchain)
    for command, version in ((["node", "--version"], "v" + expected["node"]),
                             (["npm", "--version"], expected["npm"]),
                             (["cargo", "ndk", "--version"], "cargo-ndk " + expected["cargoNdk"])):
        if run(*command, env=env, capture=True) != version:
            raise ValueError(f"requires {version}: {' '.join(command)}")
    if not run("rustc", "--version", env=env, capture=True).startswith("rustc " + expected["rust"] + " "):
        raise ValueError(f"requires Rust {expected['rust']}; select it with --rust-toolchain")
    xcode = run("xcodebuild", "-version", capture=True)
    if xcode != f"Xcode {expected['xcode']}\nBuild version {expected['xcodeBuild']}":
        raise ValueError("Xcode does not match source-lock.json")
    ndk = Path(os.environ["ANDROID_NDK_HOME"]).resolve()
    properties = dict(tuple(part.strip() for part in line.split("=", 1)) for line in (ndk / "source.properties").read_text().splitlines() if "=" in line)
    if properties.get("Pkg.Revision") != expected["androidNdk"]:
        raise ValueError("ANDROID_NDK_HOME does not match source-lock.json")
    clang = run("xcrun", "--find", "clang", capture=True)
    for key in ("SDKROOT", "CFLAGS", "CXXFLAGS", "CPPFLAGS", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
        env.pop(key, None)
    env.update({
        "PATH": str(Path(clang).parent) + os.pathsep + env["PATH"],
        "CC": clang, "CXX": run("xcrun", "--find", "clang++", capture=True),
        "AR": run("xcrun", "--find", "ar", capture=True),
        "RANLIB": run("xcrun", "--find", "ranlib", capture=True),
        "LIBRARY_PATH": "", "RUSTUP_TOOLCHAIN": toolchain,
        "CARGO_HOME": str(cache / "cargo"), "NPM_CONFIG_CACHE": str(cache / "npm"),
        "CARGO_INCREMENTAL": "0", "CARGO_BUILD_JOBS": env.get("CARGO_BUILD_JOBS", "4"),
        "ANDROID_NDK_HOME": str(ndk), "NDK_HOME": str(ndk),
    })
    return env


def build_native(source, lock, env, cache):
    env = dict(env, CARGO_TARGET_DIR=str(source / "target"))
    # C++ lifecycle patches need new npm packages, but do not change the Rust
    # image. Reuse slices only when every Rust input and build tool is identical.
    rust_inputs = run("git", "ls-tree", "-r", "HEAD", "--", "Cargo.lock", "Cargo.toml",
                      "runtimes/core", "runtimes/jsi/src", "runtimes/jsi/Cargo.toml",
                      "runtimes/jsi/build.rs", "runtimes/jsi/scripts/build-prebuilt.sh", cwd=source, capture=True)
    build_inputs = {"toolchain": lock["toolchain"], "targets": lock["targets"]}
    recipe = inspect.getsource(build_native) + inspect.getsource(build_environment)
    key = hashlib.sha256((rust_inputs + json.dumps(build_inputs, sort_keys=True) + recipe).encode()).hexdigest()
    saved = cache / "native" / key
    outputs = source / "runtimes/jsi/artifacts"
    if (saved / "hashes.json").exists():
        hashes = json.loads((saved / "hashes.json").read_text())
        for relative, expected in hashes.items():
            if digest(saved / "artifacts" / relative) != expected:
                raise ValueError(f"cached native artifact changed: {relative}")
        shutil.copytree(saved / "artifacts", outputs, dirs_exist_ok=True)
        run("bash", "scripts/assemble-prebuilt.sh", cwd=source / "runtimes/jsi", env=env)
        return {"inputSha256": key, "artifacts": hashes}
    rust_lock = digest(source / "Cargo.lock")
    sysroot = run("rustc", "--print", "sysroot", env=env, capture=True)
    for target in lock["targets"]:
        flags = [f"--remap-path-prefix={source}=/usr/src/ubrn",
                 f"--remap-path-prefix={env['CARGO_HOME']}=/usr/src/cargo",
                 f"--remap-path-prefix={sysroot}=/usr/lib/rust"]
        if target.endswith("linux-android"):
            arch = "aarch64" if target.startswith("aarch64") else "x86_64"
            archives = list(Path(env["ANDROID_NDK_HOME"]).glob(
                f"toolchains/llvm/prebuilt/darwin-x86_64/lib/clang/*/lib/linux/libclang_rt.builtins-{arch}-android.a"))
            if len(archives) != 1:
                raise ValueError(f"expected one compiler-rt builtins archive for {target}")
            # Encoded flags handle spaces in cache paths and take precedence over
            # target RUSTFLAGS. Preserve the pinned upstream script's link flags.
            flags += ["-C", "link-arg=-Wl,-z,max-page-size=16384", "-C",
                      "link-arg=-Wl,-soname,libuniffi_runtime_jsi.so", "-C", f"link-arg={archives[0]}"]
        native_env = dict(env, CARGO_ENCODED_RUSTFLAGS="\x1f".join(flags))
        run("bash", "runtimes/jsi/scripts/build-prebuilt.sh", target, cwd=source, env=native_env)
        if digest(source / "Cargo.lock") != rust_lock:
            raise ValueError("native build changed upstream Cargo.lock")
    run("bash", "scripts/assemble-prebuilt.sh", cwd=source / "runtimes/jsi", env=env)
    hashes = {path.relative_to(outputs).as_posix(): digest(path) for path in sorted(outputs.rglob("*")) if path.is_file()}
    saved.mkdir(parents=True, exist_ok=True)
    shutil.copytree(outputs, saved / "artifacts", dirs_exist_ok=True)
    (saved / "hashes.json").write_text(json.dumps(hashes, indent=2) + "\n")
    return {"inputSha256": key, "artifacts": hashes}


def inspect_package(path, name):
    with tarfile.open(path, "r:gz") as archive:
        files = {}
        for member in archive:
            item = PurePosixPath(member.name)
            if item.is_absolute() or ".." in item.parts or not item.parts or item.parts[0] != "package":
                raise ValueError(f"unsafe package path: {member.name}")
            if member.isdir():
                continue
            if not member.isfile():
                raise ValueError(f"non-file package member: {member.name}")
            data = archive.extractfile(member).read()
            for prefix in (b"/Users/", b"/Volumes/", b"/home/", b"C:\\Users\\"):
                if prefix in data:
                    raise ValueError(f"workstation path in {member.name}: {prefix!r}")
            files[item.relative_to("package").as_posix()] = data
    metadata = json.loads(files["package.json"])
    if metadata["name"] != name:
        raise ValueError(f"unexpected package identity: {metadata['name']}")
    required = ["LICENSE", "PRNS-UPSTREAM.json"]
    if name == "@ubjs/react-native":
        required += list(REQUIRED_NATIVE) + ["cpp/shim.cpp", "cpp/callbacks.cpp", "typescript/dist/index.js", "UbjsReactNative.podspec"]
    else:
        required += ["dist/esm/index.js", "dist/esm/index.d.ts", "dist/cjs/index.js"]
    for item in required:
        if item not in files:
            raise ValueError(f"missing {name} package content: {item}")
    for item, data in files.items():
        if item.endswith(".framework/Info.plist"):
            version = plistlib.loads(data).get("CFBundleShortVersionString", "")
            if re.fullmatch(r"\d+\.\d+\.\d+", version) is None:
                raise ValueError(f"framework release version must be numeric: {item}: {version}")
    if json.loads(files["PRNS-UPSTREAM.json"]) != inputs():
        raise ValueError(f"source provenance differs in {name}")
    return metadata


def build_recipe_provenance():
    recipe = {"buildScriptSha256": digest(Path(__file__))}
    # A detached export or edited recipe may have no corresponding Git commit.
    # Its exact hash is still provenance; never label it with an unrelated HEAD.
    try:
        revision = subprocess.check_output(
            ["git", "-C", str(HERE), "rev-parse", "HEAD"], stderr=subprocess.DEVNULL,
            text=True).strip()
        committed = subprocess.check_output(
            ["git", "-C", str(HERE), "show", f"{revision}:applications/tools/ubrn-vendor/vendor.py"],
            stderr=subprocess.DEVNULL)
        if hashlib.sha256(committed).hexdigest() == recipe["buildScriptSha256"]:
            recipe["buildScriptRevision"] = revision
    except (FileNotFoundError, subprocess.CalledProcessError):
        pass
    return recipe


def build(args):
    lock = inputs()
    cache = args.cache_dir.expanduser().resolve()
    cache.mkdir(parents=True, exist_ok=True)
    env = build_environment(cache, lock, args.rust_toolchain)
    source, tree = prepare(cache, lock)
    run("cargo", "fetch", "--locked", cwd=source, env=env)
    native = build_native(source, lock, env, cache)
    if args.native_only:
        print("Complete native slices cached; no packages or final receipt written.")
        return
    packages = []
    destination = VENDOR / "packages"
    destination.mkdir(exist_ok=True)
    for name, relative in lock["packages"].items():
        package = source / relative
        run("npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund", cwd=package, env=env)
        run("npm", "run", "build", cwd=package, env=env)
        if name == "@ubjs/react-native":
            run("npm", "test", cwd=package, env=env)
            run("npm", "run", "test:prebuilt", cwd=package, env=env)
        # Stage a distribution-only copy; leave the pinned source tree unchanged.
        staging = source.parent / ("pack-" + name.split("/")[-1])
        if staging.exists():
            shutil.rmtree(staging)
        shutil.copytree(package, staging, ignore=shutil.ignore_patterns("node_modules", "artifacts", "build", "target", "*.tgz"))
        shutil.copyfile(source / "LICENSE", staging / "LICENSE")
        shutil.copyfile(LOCK, staging / "PRNS-UPSTREAM.json")
        metadata = json.loads((staging / "package.json").read_text())
        metadata["files"] += ["PRNS-UPSTREAM.json"]
        (staging / "package.json").write_text(json.dumps(metadata, indent=2) + "\n")
        packed = json.loads(run("npm", "pack", "--ignore-scripts", "--json", "--pack-destination", destination, cwd=staging, env=env, capture=True))[0]
        artifact = destination / packed["filename"]
        inspect_package(artifact, name)
        packages.append({"name": name, "version": packed["version"], "file": "packages/" + packed["filename"],
                         "sha256": digest(artifact), "integrity": packed["integrity"]})
    receipt = {"schemaVersion": 2, "sourceLockSha256": digest(LOCK), **build_recipe_provenance(),
               "verificationScriptSha256": digest(Path(__file__)),
               "patchedSourceTree": tree, "nativeBuild": native, "packages": packages}
    RECEIPT.write_text(json.dumps(receipt, indent=2) + "\n")
    check()


def check():
    lock = inputs()
    receipt = json.loads(RECEIPT.read_text())
    if receipt.get("schemaVersion") != 2:
        raise ValueError("unsupported vendor receipt version")
    # The build hash records what produced the archives. A reviewed verifier
    # update may retain those exact bytes without claiming a new native build.
    if re.fullmatch(r"[0-9a-f]{64}", receipt.get("buildScriptSha256", "")) is None:
        raise ValueError("receipt must identify its historical build recipe")
    revision = receipt.get("buildScriptRevision")
    if revision is not None and re.fullmatch(r"[0-9a-f]{40}", revision) is None:
        raise ValueError("historical build recipe revision must be a full Git commit")
    if receipt.get("verificationScriptSha256") != digest(Path(__file__)):
        raise ValueError("vendor verifier changed; review it and update verificationScriptSha256")
    if receipt["sourceLockSha256"] != digest(LOCK):
        raise ValueError("vendor source inputs changed; rebuild and review the generated packages")
    if {p["name"] for p in receipt["packages"]} != set(lock["packages"]):
        raise ValueError("receipt does not contain both runtime packages")
    for package in receipt["packages"]:
        path = VENDOR / package["file"]
        integrity = "sha512-" + base64.b64encode(hashlib.sha512(path.read_bytes()).digest()).decode()
        if digest(path) != package["sha256"] or integrity != package["integrity"]:
            raise ValueError(f"package digest mismatch: {path.name}")
        if inspect_package(path, package["name"])["version"] != package["version"]:
            raise ValueError(f"package version differs from receipt: {path.name}")
        print(f"verified {package['name']}@{package['version']}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check", help="verify committed archives without native toolchains or install scripts")
    builder = commands.add_parser("build", help="rebuild both complete packages on macOS")
    builder.add_argument("--cache-dir", type=Path, required=True, help="disposable source, Cargo, npm and build cache directory")
    builder.add_argument("--rust-toolchain", default="stable", help="rustup name resolving to the version in source-lock.json")
    builder.add_argument("--native-only", action="store_true", help="cache complete native slices without sealing npm packages")
    args = parser.parse_args()
    try:
        build(args) if args.command == "build" else check()
    except (ValueError, KeyError, FileNotFoundError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"ubrn vendor: {error}\n")
