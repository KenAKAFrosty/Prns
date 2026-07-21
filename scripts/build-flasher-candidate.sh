#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
candidate="${1:-}"
channel="${2:-preview}"
key_id="${3:-}"
commit="${4:-$(git -C "$root" rev-parse HEAD)}"
if [[ -z "$candidate" || -z "$key_id" ]]; then
    echo "usage: scripts/build-flasher-candidate.sh OUTPUT_DIR stable|preview KEY_ID [SOURCE_COMMIT]" >&2
    exit 2
fi
candidate="$(python3 "$root/scripts/flasher_candidate_output.py" "$root" "$candidate")"
case "$channel" in
    stable|preview) ;;
    *) echo "channel must be stable or preview" >&2; exit 2 ;;
esac
if [[ -e "$candidate" ]] && [[ -n "$(find "$candidate" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)" ]]; then
    echo "candidate output must be a new or empty directory: $candidate" >&2
    exit 2
fi
if [[ "$commit" != "$(git -C "$root" rev-parse HEAD)" ]]; then
    echo "source commit must equal the checked-out HEAD" >&2
    exit 2
fi
source_date_epoch="$(git -C "$root" show -s --format=%ct "$commit")"
if [[ ! "$source_date_epoch" =~ ^[1-9][0-9]*$ ]]; then
    echo "source commit has no valid deterministic timestamp" >&2
    exit 2
fi
if [[ -n "${SOURCE_DATE_EPOCH:-}" && "$SOURCE_DATE_EPOCH" != "$source_date_epoch" ]]; then
    echo "SOURCE_DATE_EPOCH must equal the checked-out source commit timestamp" >&2
    exit 2
fi
export SOURCE_DATE_EPOCH="$source_date_epoch"
if [[ -n "$(git -C "$root" status --porcelain)" ]]; then
    echo "release candidates must be built from a clean checkout" >&2
    exit 2
fi
for release_input in \
    "$root/Cargo.lock" \
    "$root/docs/website/package-lock.json" \
    "$root/LICENSE-APACHE" \
    "$root/LICENSE-MIT" \
    "$root/THIRD_PARTY_NOTICES.md" \
    "$root/release/licenses/pako-Zlib.txt" \
    "$root/release/licenses/spark-md5-MIT.txt"; do
    if [[ ! -s "$release_input" ]]; then
        echo "required locked release input is missing or empty: $release_input" >&2
        exit 2
    fi
done
if rg -q 'PRNS_RELEASE_KEY_NOT_CONFIGURED' "$root/release/keys/minisign.pub"; then
    echo "pin the maintainer-controlled Minisign public key before building a candidate" >&2
    exit 4
fi
pinned_key_id="$(sed -n '1s/^untrusted comment: minisign public key //p' "$root/release/keys/minisign.pub")"
if [[ ! "$key_id" =~ ^[0-9A-Fa-f]{16}$ ]] || [[ -z "$pinned_key_id" ]] || [[ "$(printf '%s' "$pinned_key_id" | tr '[:lower:]' '[:upper:]')" != "$(printf '%s' "$key_id" | tr '[:lower:]' '[:upper:]')" ]]; then
    echo "requested key ID does not match release/keys/minisign.pub" >&2
    exit 4
fi
dx_version="$(dx --version)"
if [[ "$dx_version" != *"0.7.5"* ]]; then
    echo "dioxus-cli 0.7.5 is required" >&2
    exit 2
fi

version="$(tr -d '[:space:]' < "$root/VERSION")"
if [[ "$(cargo run --quiet --locked -p hopspot-flash -- --version)" != "hopspot-flash $version" ]]; then
    echo "hopspot-flash package version must equal repository VERSION" >&2
    exit 2
fi
mkdir -p "$candidate" "$candidate/metadata"
cp "$root/VERSION" "$candidate/VERSION"
cp "$root/THIRD_PARTY_NOTICES.md" "$candidate/THIRD_PARTY_NOTICES.md"
cp "$root/LICENSE-APACHE" "$candidate/LICENSE-APACHE"
cp "$root/LICENSE-MIT" "$candidate/LICENSE-MIT"
cp "$root/release/keys/minisign.pub" "$candidate/minisign.pub"
python3 "$root/scripts/write-flasher-build-metadata.py" \
    --output "$candidate/metadata/build.json" \
    --commit "$commit" \
    --source-date-epoch "$source_date_epoch"

cd "$root/docs/website"
if [[ -e public/firmware || -e public/assets/flasher || -e public/flash-manifest.json ]]; then
    echo "legacy generated hosted flasher assets remain under docs/website/public; clean them before a candidate build" >&2
    exit 2
fi
npm ci --ignore-scripts --no-audit --no-fund
npm run test:flasher
npm run build:css
npm run build:flasher
git -C "$root" diff --exit-code -- docs/website/public/assets/tailwind.css

embedded_dist="$root/docs/website/target/dx/reticulum-site/release/web/public"
boundary_root="$root/docs/website/target/flasher-production-boundary"
case "$embedded_dist" in
    "$root/docs/website/target/dx/reticulum-site/"*) ;;
    *) echo "refusing to clear unexpected Dioxus output path" >&2; exit 2 ;;
esac
case "$boundary_root" in
    "$root/docs/website/target/flasher-production-boundary") ;;
    *) echo "refusing unexpected production-boundary path" >&2; exit 2 ;;
esac
rm -rf -- "$embedded_dist"
PRNS_EMBEDDED_SITE=1 \
PRNS_BUILD_VERSION="$version" \
PRNS_BUILD_COMMIT="$commit" \
PRNS_BUILD_CHANNEL="$channel" \
dx build --platform web --debug-symbols false --release --locked --features embedded-site

test -f "$embedded_dist/index.html"
rm -rf -- "$boundary_root"
mkdir -p "$boundary_root/embedded"
cp -R "$embedded_dist/." "$boundary_root/embedded/"
if rg -l -i 'esptool-js|esp-web-install-button|unpkg|prns-flash\.js' "$embedded_dist"; then
    echo "embedded SoftAP site unexpectedly contains hosted flashing JavaScript" >&2
    exit 1
fi
if find "$embedded_dist" \( -path '*/firmware/*' -o -path '*/assets/flasher/*' \) -print -quit | rg -q .; then
    echo "embedded SoftAP site unexpectedly contains hosted firmware or flasher assets" >&2
    exit 1
fi

cd "$root"
for board in heltec-v4 t-beam-supreme xiao-esp32-c6 t-echo; do
    PRNS_EMBEDDED_SITE_READY=1 cargo run --locked -p hopspot-flash -- build "$board" --out-root "$candidate"
done
cargo run --locked -p hopspot-flash -- assemble-manifest \
    --out-root "$candidate" \
    --channel "$channel" \
    --commit "$commit" \
    --key-id "$key_id"

cd "$root/docs/website"
rm -rf -- "$embedded_dist"
PRNS_BUILD_VERSION="$version" \
PRNS_BUILD_COMMIT="$commit" \
PRNS_BUILD_CHANNEL="$channel" \
PRNS_WRITE_PUBLIC_ASSETS=1 \
dx build --platform web --debug-symbols false --release --locked

hosted_dist="$root/docs/website/target/dx/reticulum-site/release/web/public"
test -f "$hosted_dist/index.html"
mkdir -p "$candidate/website/assets/flasher"
cp -R "$hosted_dist/." "$candidate/website/"
cp "$root/docs/website/target/hosted-assets/prns-flash.js" \
    "$candidate/website/assets/flasher/prns-flash.js"
cp "$root/THIRD_PARTY_NOTICES.md" "$candidate/website/THIRD_PARTY_NOTICES.md"
bash "$root/docs/website/tools/verify-web-flasher-production-boundary.sh" \
    "$candidate/website" \
    "$boundary_root/embedded"
cd "$root"
cargo doc --locked --no-deps --workspace
mkdir -p "$candidate/website/api"
cp -R "$root/target/doc/." "$candidate/website/api/"
cp "$root/release/website/api-index.html" "$candidate/website/api/index.html"

python3 "$root/scripts/npm-production-audit.py"
if rg -i 'esp-web-install-button|unpkg\.com|esp-web-tools|playwright|axe-core' \
    "$root/docs/website/target/hosted-assets/prns-flash.js"; then
    echo "production bundle contains a forbidden legacy/CDN/test-only dependency" >&2
    exit 1
fi
git -C "$root" diff --exit-code -- THIRD_PARTY_NOTICES.md docs/website/public/assets/tailwind.css

echo "Built unsigned core flasher candidate $version at $candidate"
echo "Add all five CLI archives, then run scripts/finalize-flasher-candidate.py."
