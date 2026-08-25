#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
candidate="${1:-}"
channel="${2:-preview}"
key_id="${3:-}"
commit="${4:-$(git -C "$root" rev-parse HEAD)}"
history="${PRNS_RELEASE_HISTORY:-}"
if [[ -z "$candidate" || -z "$key_id" ]]; then
    echo "usage: tools/release/build-flasher-candidate.sh OUTPUT_DIR stable|preview KEY_ID [SOURCE_COMMIT]" >&2
    exit 2
fi
if [[ -z "$history" || ! -f "$history/history.json" ]]; then
    echo "PRNS_RELEASE_HISTORY must name one verified bootstrap or retained history input" >&2
    exit 2
fi
history="$(cd "$history" && pwd)"
candidate="$(python3 "$root/tools/release/flasher_candidate_output.py" "$root" "$candidate")"
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
if grep -qF 'PRNS_RELEASE_KEY_NOT_CONFIGURED' "$root/release/keys/minisign.pub"; then
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
if [[ "$(wasm-bindgen --version)" != "wasm-bindgen 0.2.126" ]]; then
    echo "wasm-bindgen 0.2.126 is required" >&2
    exit 2
fi

suite_version="$(tr -d '[:space:]' < "$root/VERSION")"
version="${PRNS_FLASH_VERSION:-$suite_version}"
python3 "$root/tools/release/flasher_hotfix.py" identity \
    --repository "$root" --version "$version" >/dev/null
export PRNS_FLASH_VERSION="$version"
if [[ "$(cargo run --quiet --locked -p hopspot-flash -- --version)" != "hopspot-flash $version" ]]; then
    echo "hopspot-flash compiled version must equal the requested flasher release version" >&2
    exit 2
fi
roster_source="$root/release/acceptance/rosters/${suite_version}.json"
if ! git -C "$root" ls-files --error-unmatch "release/acceptance/rosters/${suite_version}.json" >/dev/null 2>&1; then
    echo "a committed tester roster is required for release $version: $roster_source" >&2
    exit 2
fi
python3 "$root/tools/release/validate-flasher-tester-roster.py" \
    --roster "$roster_source" \
    --version "$suite_version"

mkdir -p "$candidate" "$candidate/metadata" "$candidate/qualification" "$candidate/website"
printf '%s\n' "$version" > "$candidate/VERSION"
cp "$root/THIRD_PARTY_NOTICES.md" "$candidate/THIRD_PARTY_NOTICES.md"
cp "$root/LICENSE-APACHE" "$candidate/LICENSE-APACHE"
cp "$root/LICENSE-MIT" "$candidate/LICENSE-MIT"
cp "$root/release/keys/minisign.pub" "$candidate/minisign.pub"
cp "$root/release/acceptance/QUALIFICATION.md" \
    "$candidate/qualification/QUALIFICATION.md"
cp "$root/tools/release/create-flasher-acceptance.py" \
    "$candidate/qualification/create-flasher-acceptance.py"
cp "$root/tools/release/validate-flasher-acceptance.py" \
    "$candidate/qualification/validate-flasher-acceptance.py"
cp "$root/tools/release/flasher_acceptance_contract.py" \
    "$candidate/qualification/flasher_acceptance_contract.py"
cp "$root/tools/release/flasher_manifest.py" \
    "$candidate/qualification/flasher_manifest.py"
cp "$root/tools/release/flasher_tester_roster.py" \
    "$candidate/qualification/flasher_tester_roster.py"
cp "$root/tools/release/flasher_hotfix.py" \
    "$candidate/qualification/flasher_hotfix.py"
cp "$root/tools/release/package-flasher-qualification-evidence.py" \
    "$candidate/qualification/package-flasher-qualification-evidence.py"
cp "$root/tools/release/serve-flasher-candidate.py" \
    "$candidate/qualification/serve-flasher-candidate.py"
cp "$root/tools/release/verify-flasher-candidate-files.py" \
    "$candidate/qualification/verify-flasher-candidate-files.py"
cp "$root/tools/release/validate-flasher-tester-roster.py" \
    "$candidate/qualification/validate-flasher-tester-roster.py"
cp "$roster_source" "$candidate/qualification/tester-roster.json"
if [[ "$version" != "$suite_version" ]]; then
    hotfix_spec="$root/release/flash/hotfixes/${version}.json"
    if ! git -C "$root" ls-files --error-unmatch \
        "release/flash/hotfixes/${version}.json" >/dev/null 2>&1; then
        echo "a committed hotfix specification is required: $hotfix_spec" >&2
        exit 2
    fi
    cp "$hotfix_spec" "$candidate/qualification/hotfix.json"
fi
python3 "$root/tools/release/write-flasher-build-metadata.py" \
    --output "$candidate/metadata/build.json" \
    --commit "$commit" \
    --source-date-epoch "$source_date_epoch"
python3 "$root/tools/release/package-source-snapshot.py" \
    --repository "$root" \
    --commit "$commit" \
    --version "$suite_version" \
    --output "$candidate/website/source.zip" \
    --metadata "$candidate/metadata/source.json"
export PRNS_SOURCE_ARCHIVE="$candidate/website/source.zip"
export PRNS_SOURCE_VERSION="$suite_version"
export PRNS_SOURCE_COMMIT="$commit"
export PRNS_SOURCE_SIZE
PRNS_SOURCE_SIZE="$(wc -c < "$PRNS_SOURCE_ARCHIVE" | tr -d '[:space:]')"
export PRNS_SOURCE_SHA256
PRNS_SOURCE_SHA256="$(cut -d ' ' -f 1 "$candidate/website/source.zip.sha256")"

cd "$root/docs/website"
for legacy_public_path in \
    public/firmware \
    public/assets/flasher \
    public/flash-manifest.json \
    public/source.zip \
    public/source.zip.sha256; do
    if [[ -e "$legacy_public_path" || -L "$legacy_public_path" ]]; then
        echo "legacy generated hosted asset remains; clean it before a candidate build: $legacy_public_path" >&2
        exit 2
    fi
done
npm ci --ignore-scripts --no-audit --no-fund
npm run test:flasher
npm run build:css
npm run build:flasher
git -C "$root" diff --exit-code -- docs/website/public/assets/tailwind.css

dioxus_dist="$root/docs/website/target/dx/reticulum-site/release/web/public"
captive_page="$root/personal-hopspot/embedded/esp32/assets/captive-portal.html"
boundary_root="$root/docs/website/target/flasher-production-boundary"
case "$dioxus_dist" in
    "$root/docs/website/target/dx/reticulum-site/"*) ;;
    *) echo "refusing to clear unexpected Dioxus output path" >&2; exit 2 ;;
esac
case "$boundary_root" in
    "$root/docs/website/target/flasher-production-boundary") ;;
    *) echo "refusing unexpected production-boundary path" >&2; exit 2 ;;
esac
test -f "$captive_page"
rm -rf -- "$boundary_root"
mkdir -p "$boundary_root/embedded"
cp "$captive_page" "$boundary_root/embedded/index.html"
if grep -R -a -l -i -E 'esptool-js|esp-web-install-button|unpkg|prns-flash\.js' "$boundary_root/embedded"; then
    echo "embedded SoftAP site unexpectedly contains hosted flashing JavaScript" >&2
    exit 1
fi
if find "$boundary_root/embedded" \( -path '*/firmware/*' -o -path '*/assets/flasher/*' \) -print -quit | grep -q .; then
    echo "embedded SoftAP site unexpectedly contains hosted firmware or flasher assets" >&2
    exit 1
fi

cd "$root"
if [[ "$version" == "$suite_version" ]]; then
    "$root/tools/prns" release firmware build -- --all "$candidate"
else
    while IFS= read -r board; do
        "$root/tools/prns" release firmware build -- "$board" "$candidate"
    done < <(
        "$root/tools/prns" release hotfix -- identity \
            --repository "$root" --version "$version" --format changed-boards
    )
    "$root/tools/prns" release hotfix -- compose \
        --repository "$root" \
        --history "$history" \
        --candidate "$candidate" \
        --version "$version"
fi
cargo run --locked -p hopspot-flash -- assemble-manifest \
    --out-root "$candidate" \
    --channel "$channel" \
    --commit "$commit" \
    --key-id "$key_id"

cd "$root/docs/website"
rm -rf -- "$dioxus_dist"
PRNS_BUILD_VERSION="$version" \
PRNS_BUILD_COMMIT="$commit" \
PRNS_BUILD_CHANNEL="$channel" \
PRNS_API_DOCS_STAGED=1 \
dx build --platform web --debug-symbols false --release --locked

hosted_dist="$dioxus_dist"
test -f "$hosted_dist/index.html"
mkdir -p "$candidate/website/assets/flasher"
cp -R "$hosted_dist/." "$candidate/website/"
cp "$candidate/website/index.html" "$candidate/website/404.html"
npm --prefix "$root/prns-wasm" ci --ignore-scripts --no-audit --no-fund
bash "$root/tools/build/stage-wasm-docs-browser-playground.sh" \
    "$candidate/website/browser-node-playground-console"
cp "$root/docs/website/target/hosted-assets/prns-flash.js" \
    "$candidate/website/assets/flasher/prns-flash.js"
cp -R "$root/docs/website/target/hosted-assets/nrf-dfu" \
    "$candidate/website/assets/flasher/nrf-dfu"
cp "$root/THIRD_PARTY_NOTICES.md" "$candidate/website/THIRD_PARTY_NOTICES.md"
python3 "$root/tools/release/flasher-website-history.py" apply \
    --history "$history" \
    --candidate "$candidate"
bash "$root/docs/website/tools/verify-web-flasher-production-boundary.sh" \
    "$candidate/website" \
    "$boundary_root/embedded"
cd "$root"
rustdoc_packages="$root/target/flasher-rustdoc-workspace-packages.txt"
cargo metadata --locked --no-deps --format-version 1 |
    python3 "$root/tools/release/flasher_rustdoc.py" \
        --list-workspace-packages > "$rustdoc_packages"
test -s "$rustdoc_packages"
rm -rf -- "$root/target/doc"
while IFS= read -r package; do
    cargo doc --locked --no-deps --package "$package" --jobs 1
done < "$rustdoc_packages"
python3 "$root/tools/release/flasher_rustdoc.py" \
    "$root/target/doc" \
    --current-crate docs
mkdir -p "$candidate/website/api"
cp -R "$root/target/doc/." "$candidate/website/api/"
cp "$root/release/website/api-index.html" "$candidate/website/api/index.html"

python3 "$root/validation/security/npm-production-audit.py"
if grep -i -E 'esp-web-install-button|unpkg\.com|esp-web-tools|playwright|axe-core' \
    "$root/docs/website/target/hosted-assets/prns-flash.js"; then
    echo "production bundle contains a forbidden legacy/CDN/test-only dependency" >&2
    exit 1
fi
git -C "$root" diff --exit-code -- THIRD_PARTY_NOTICES.md docs/website/public/assets/tailwind.css

echo "Built unsigned core flasher candidate $version at $candidate"
echo "Add all five CLI archives, then run tools/release/finalize-flasher-candidate.py."
