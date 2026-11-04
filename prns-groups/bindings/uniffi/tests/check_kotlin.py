#!/usr/bin/env python3
"""Compile all independent generated Kotlin namespaces and run their JVM consumer.

Requires Gradle 9.3.1 and Java 17 or later, without Android, Expo, npm or the RN
binding generator. PRNS_KOTLIN_GRADLE may select an existing Gradle executable;
PRNS_KOTLIN_OFFLINE=1 restricts Gradle to its already populated dependency cache.
"""
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[4]
BINDINGS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools/uniffi'))
import tooling


def checksum_probe(files):
    """Execute the actual normalized check body with controlled JNA carriers.

    Derive symbols/expected values from generated output, with no domain method
    inventory. The real image still exercises its ordinary library initializer.
    This probe supplies upper bits that a platform ABI may leave unspecified.
    """
    for source in files.values():
        match = tooling._KOTLIN_CHECKSUM_FUNCTION.search(source)
        if match is None:
            continue
        checks = re.findall(
            r'lib\.(uniffi_\w+_checksum_\w+)\(\)\.toUShort\(\)\.toInt\(\) != ([0-9]+)',
            match[0])
        if any(int(expected) >= 0x8000 for _, expected in checks):
            break
    else:
        raise ValueError('generated Kotlin omitted normalized high-bit checksum coverage')
    methods = '\n'.join(
        f'    fun {name}(): Int = carrier({expected})' for name, expected in checks)
    return """// Test-only carriers; the validation function below is copied from actual generated output.
package prns.checksum.regression

private class IntegrityCheckingUniffiLib(private val mode: Int) {
    private fun carrier(expected: Int): Int = when (mode) {
        0 -> expected
        1 -> expected.toShort().toInt()
        2 -> expected or 0x5a5a0000
        3 -> expected or 0xa5a50000.toInt()
        4 -> (expected xor 1) or 0x5a5a0000
        else -> (expected xor 1) or 0xa5a50000.toInt()
    }
""" + methods + '\n}\n\n' + match[0] + """

fun verifyChecksumCarriers() {
    for (mode in 0..3) uniffiCheckApiChecksums(IntegrityCheckingUniffiLib(mode))
    for (mode in 4..5) {
        val failure = runCatching { uniffiCheckApiChecksums(IntegrityCheckingUniffiLib(mode)) }.exceptionOrNull()
        check(failure is RuntimeException && failure.message ==
            "UniFFI API checksum mismatch: try cleaning and rebuilding your project")
    }
    println("KOTLIN_CHECKSUM_CARRIERS_OK")
}
"""


def main():
    env = tooling.environment(ROOT / 'target')
    gradle = os.environ.get('PRNS_KOTLIN_GRADLE', 'gradle')
    version = subprocess.check_output([gradle, '--version'], env=env, text=True)
    if '\nGradle 9.3.1\n' not in version:
        raise SystemExit('standalone Kotlin qualification requires Gradle 9.3.1')
    print(version, flush=True)  # Retain JVM/Gradle toolchain provenance with the gate log.
    target = Path(env['CARGO_TARGET_DIR']).resolve()
    env['CARGO_TARGET_DIR'] = str(target)
    target.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='group-kotlin-', dir=target) as temporary:
        project = Path(temporary)
        source = BINDINGS / 'tests/kotlin'
        for name in ('settings.gradle.kts', 'build.gradle.kts'):
            shutil.copyfile(source / name, project / name)
        (project / 'consumer').mkdir()
        shutil.copyfile(source / 'Consumer.kt', project / 'consumer/Consumer.kt')
        image = BINDINGS / 'image'
        recipe = tooling.BindingRecipe(
            manifest=image / 'Cargo.toml', package='prns-groups-uniffi-image',
            library_name='prns_group_services', crate_dir=image,
            kotlin_dir=project / 'generated', features=('uniffi-bindgen',),
        )
        files = tooling.generated_files(recipe, None, env)
        (project / 'consumer/ChecksumCarriers.kt').write_text(checksum_probe(files))
        for path, content in files.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        subprocess.run([
            gradle, '--no-daemon', '--max-workers=2',
            *(['--offline'] if env.get('PRNS_KOTLIN_OFFLINE') == '1' else []),
            '-Dorg.gradle.jvmargs=-Xmx2048m -XX:MaxMetaspaceSize=768m',
            '-Pkotlin.compiler.execution.strategy=in-process',
            '-PnativeLibraryDirectory=' + str(target / 'debug'),
            'run',
        ], cwd=project, env=env, check=True, timeout=600)


if __name__ == '__main__':
    main()
