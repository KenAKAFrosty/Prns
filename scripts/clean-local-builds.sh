#!/usr/bin/env sh
set -eu

usage() {
    echo "usage: sh scripts/clean-local-builds.sh [--dry-run|--apply]" >&2
}

mode="${1:---dry-run}"
case "$mode" in
    --dry-run) apply=0 ;;
    --apply) apply=1 ;;
    *) usage; exit 2 ;;
esac

paths="
target
fuzz/target
fuzz/artifacts
fuzz/coverage
rvt/target
rvt/dist
rvt/.dx
docs/website/target
docs/website/dist
docs/website/node_modules
hosts/esp32-c6/target
hosts/heltec-lora32/target
hosts/nrf52840/target
android-aar/.gradle
android-aar/build
android-aar/lib/build
"

if [ "$apply" -eq 0 ]; then
    echo "Local build artifacts that would be removed:"
fi

for path in $paths; do
    if [ -e "$path" ]; then
        if [ "$apply" -eq 1 ]; then
            rm -rf -- "$path"
            echo "removed $path"
        else
            du -sh "$path"
        fi
    fi
done

if [ "$apply" -eq 0 ]; then
    echo "Run with --apply to remove these ignored build artifacts."
fi
