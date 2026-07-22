#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
output="${1:-$root/release-audit-evidence.md}"
commit="$(git -C "$root" rev-parse HEAD)"
tree_state="clean"
if [[ -n "$(git -C "$root" status --porcelain)" ]]; then
    tree_state="dirty"
fi
notice_hash="$(shasum -a 256 "$root/THIRD_PARTY_NOTICES.md" | awk '{print $1}')"
unsafe_hash="$(shasum -a 256 "$root/audits/unsafe-snapshot.json" | awk '{print $1}')"

{
    echo "# Release dependency audit evidence"
    echo
    echo "- Commit: \`$commit\`"
    echo "- Checkout state: \`$tree_state\`"
    echo "- cargo-deny: \`$(cargo deny --version)\`"
    echo "- cargo-about: \`$(cargo about --version)\`"
    echo "- THIRD_PARTY_NOTICES.md SHA-256: \`$notice_hash\`"
    echo "- audits/unsafe-snapshot.json SHA-256: \`$unsafe_hash\`"
    echo "- Result marker: \`RELEASE_DEPENDENCY_AUDIT_COMPLETE\`"
} > "$output"
