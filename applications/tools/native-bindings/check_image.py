#!/usr/bin/env python3
"""Compile every generated Swift namespace and call the selected app image."""
from pathlib import Path
import subprocess
import sys
import tempfile

import generate

def main():
    if sys.platform != 'darwin':
        raise SystemExit('app Swift image check requires macOS and Xcode command-line tools')
    env = generate.tooling.environment(generate.APPLICATIONS / 'target')
    target = Path(env['CARGO_TARGET_DIR']).resolve()
    target.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='app-image-swift-', dir=target) as temporary:
        output = Path(temporary)
        provider = generate.sdk.image_provider(generate.IMAGE)
        recipe = generate.tooling.BindingRecipe(
            manifest=provider['manifest'], package=provider['package'],
            library_name=provider['libraryName'], crate_dir=provider['crateDirectory'],
            swift_dir=output, features=tuple(provider['features'] + provider['generationFeatures']))
        files = generate.tooling.generated_files(recipe, None, env)
        for path, content in files.items():
            path.write_text(content)
        executable = generate.tooling.compile_swift_consumer(
            generated_sources=sorted(output.glob('*.swift')),
            module_maps=sorted(output.glob('*.modulemap')), module_name='PrnsChatAppImage',
            native_library=target / 'debug/libprns_chat_app.dylib',
            fixture=generate.HERE / 'image_conformance.swift', output_dir=output, env=env)
        subprocess.run([str(executable), str(generate.app_api_version())],
                       env=env, check=True, timeout=30)

if __name__ == '__main__':
    main()
