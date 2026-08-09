#!/usr/bin/env python3

import argparse
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser()
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--repository")
    source.add_argument("--repository-url")
    parser.add_argument("--library", required=True)
    args = parser.parse_args()
    repository = (
        Path(args.repository).resolve() if args.repository is not None else None
    )
    library = Path(args.library).resolve()
    version = (ROOT / "VERSION").read_text().strip()
    if repository is not None:
        jar = (
            repository
            / "io"
            / "reticulum"
            / "personal-rns"
            / version
            / f"personal-rns-{version}.jar"
        )
    else:
        jar = None
    if (jar is not None and not jar.is_file()) or not library.is_file():
        raise SystemExit("staged Maven package or native library is missing")
    with tempfile.TemporaryDirectory(prefix="prns-maven-package-") as temporary:
        consumer = Path(temporary)
        source = consumer / "src" / "main" / "java"
        source.mkdir(parents=True)
        (consumer / "settings.gradle.kts").write_text(
            'rootProject.name = "personal-rns-package-smoke"\n'
        )
        repository_uri = (
            repository.as_uri()
            if repository is not None
            else args.repository_url
        )
        library_path = str(library).replace("\\", "\\\\").replace('"', '\\"')
        (consumer / "build.gradle.kts").write_text(
            "plugins {\n"
            "    application\n"
            "}\n"
            "\n"
            "repositories {\n"
            f"    maven {{ url = uri(\"{repository_uri}\") }}\n"
            "    mavenCentral()\n"
            "}\n"
            "\n"
            "dependencies {\n"
            f'    implementation("rs.reticulum:personal-rns:{version}")\n'
            "}\n"
            "\n"
            "application {\n"
            '    mainClass = "PackageSmoke"\n'
            "    applicationDefaultJvmArgs = listOf(\n"
            f'        "-Dpersonal.rns.library={library_path}"\n'
            "    )\n"
            "}\n"
        )
        (source / "PackageSmoke.java").write_text(
            "import rs.reticulum.prns.Host;\n"
            "import rs.reticulum.prns.HostOptions;\n"
            "import rs.reticulum.prns.HostRole;\n"
            "import rs.reticulum.prns.IdentityConfigGenerateEphemeral;\n"
            "import rs.reticulum.prns.Limits;\n"
            "import java.util.Collections;\n"
            "\n"
            "public final class PackageSmoke {\n"
            "    public static void main(String[] arguments) {\n"
            "        HostOptions options = new HostOptions(\n"
            "            HostRole.ENDPOINT,\n"
            "            IdentityConfigGenerateEphemeral.INSTANCE,\n"
            "            Collections.emptyList(),\n"
            "            Collections.emptySet(),\n"
            "            new Limits(64L, 256L, 8388608L, 1024L)\n"
            "        );\n"
            "        try (Host host = new Host(options)) {\n"
            "            if (host == null) {\n"
            '                throw new AssertionError("host creation failed");\n'
            "            }\n"
            "        }\n"
            "    }\n"
            "}\n"
        )
        command = [
            str(ROOT / "prns-host" / "bindings" / "jvm" / "gradlew"),
            "--project-dir",
            str(consumer),
            "--no-daemon",
        ]
        if repository is not None:
            command.append("--offline")
        command.extend(["--stacktrace", "run"])
        subprocess.run(
            command,
            cwd=consumer,
            check=True,
        )


if __name__ == "__main__":
    main()
