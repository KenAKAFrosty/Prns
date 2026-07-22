cargo_target_directory() {
    local manifest="$1"
    cargo metadata --no-deps --format-version 1 --manifest-path "$manifest" \
        | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
}

cargo_debug_binary() {
    local manifest="$1"
    local binary="$2"
    printf '%s/debug/%s\n' "$(cargo_target_directory "$manifest")" "$binary"
}

cargo_debug_example() {
    local manifest="$1"
    local example="$2"
    printf '%s/debug/examples/%s\n' "$(cargo_target_directory "$manifest")" "$example"
}
