#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/validation/.venv/rns-rpc-1.4.2/bin/python}"
ORACLE="$ROOT/validation/interop/peers/rns_rnid_oracle.py"
WORK="$(mktemp -d)"
PRIVATE="$WORK/oracle.rid"
PUBLIC="$WORK/oracle.pub"
STOCK_MESSAGE="$WORK/stock-message"
STOCK_SIGNATURE="$STOCK_MESSAGE.rsg"
CANDIDATE_MESSAGE="$WORK/candidate-message"
PLAINTEXT="$WORK/plaintext"
STOCK_ENCRYPTED="$WORK/stock.rfe"
ARTIFACT_MESSAGE="$WORK/stock-artifact-message"
STOCK_RSG="$ARTIFACT_MESSAGE.rsg"
PRNS_ARTIFACT_MESSAGE="$WORK/prns-artifact-message"
STOCK_RSM="$WORK/stock.rsm"
PRNS_RSM="$WORK/prns.rsm"
METADATA="$WORK/metadata"
METADATA_SPEC="$WORK/metadata.spec"
BIN="$ROOT/prnsd/target/debug/prnsd"

cleanup() {
    rm -rf -- "$WORK"
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }

HASH="$($PYTHON "$ORACLE" prepare "$PRIVATE" "$PUBLIC" "$STOCK_MESSAGE" "$STOCK_SIGNATURE" "$PLAINTEXT" "$STOCK_ENCRYPTED")"
$PYTHON "$ORACLE" prepare-artifacts "$PRIVATE" "$ARTIFACT_MESSAGE" "$STOCK_RSG" "$STOCK_RSM"
cp "$ARTIFACT_MESSAGE" "$PRNS_ARTIFACT_MESSAGE"
cp "$STOCK_MESSAGE" "$CANDIDATE_MESSAGE"
( cd "$ROOT/prnsd" && cargo build --quiet --locked )

PRINTED="$($BIN id -i "$PRIVATE" -p -P)"
[[ "$PRINTED" == *"Identity Hash : <$HASH>"* ]] || { echo "FAIL: Prnsd printed the wrong identity hash"; exit 1; }
[[ "$PRINTED" == *"Public Key    : 0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"* ]] || { echo "FAIL: Prnsd printed the wrong public key"; exit 1; }

HASHED="$($BIN id -i "$PRIVATE" -H example.aspect)"
[[ "$HASHED" == *"<f6f4f4a9b02bf85109dbf53265984547>"* ]] || { echo "FAIL: Prnsd derived the wrong destination hash"; exit 1; }
[[ "$HASHED" == *"<example.aspect.$HASH:f6f4f4a9b02bf85109dbf53265984547>"* ]] || { echo "FAIL: Prnsd rendered the wrong full destination specifier"; exit 1; }

$BIN id -i "$PRIVATE" -w "$WORK/exported" > "$WORK/export.out"
cmp "$PUBLIC" "$WORK/exported.pub"

$BIN id -i "$PRIVATE" -s "$CANDIDATE_MESSAGE" --raw > "$WORK/sign.out"
$PYTHON "$ORACLE" verify "$PRIVATE" "$CANDIDATE_MESSAGE" "$CANDIDATE_MESSAGE.rsg"
$BIN id -m "$PUBLIC" -V "$STOCK_MESSAGE" > "$WORK/validate.out"

$BIN id -i "$PRIVATE" -s "$PRNS_ARTIFACT_MESSAGE" > "$WORK/artifact-sign.out"
$PYTHON "$ORACLE" verify-rsg "$PRIVATE" "$PRNS_ARTIFACT_MESSAGE" "$PRNS_ARTIFACT_MESSAGE.rsg"
$BIN id -V "$ARTIFACT_MESSAGE" > "$WORK/artifact-validate.out"
for ENCODING in base32 base64 base256 hex; do
    case "$ENCODING" in
        base32) FLAG=-B ;;
        base64) FLAG=-b ;;
        base256) FLAG=-U ;;
        hex) FLAG=-F ;;
    esac
    $BIN id -i "$PRIVATE" -s "$PRNS_ARTIFACT_MESSAGE" "$FLAG" > "$WORK/artifact-$ENCODING.out"
    $PYTHON "$ORACLE" verify-encoded-rsg "$PRIVATE" "$PRNS_ARTIFACT_MESSAGE" "$WORK/artifact-$ENCODING.out" "$ENCODING"
done

printf 'name = Prns\nversion = 3\ntags = one, two\nstable = yes\n' > "$METADATA"
printf 'name = string()\nversion = integer()\ntags = string_list()\nstable = boolean()\n' > "$METADATA_SPEC"
$BIN id -i "$PRIVATE" -S canonical-message-oracle -E "$METADATA" --meta-spec "$METADATA_SPEC" -w "$WORK/prns" > "$WORK/message-sign.out"
$PYTHON "$ORACLE" verify-rsm "$PRIVATE" "$PRNS_RSM"
$BIN id -V "$STOCK_RSM" --meta > "$WORK/message-validate.out"
grep -q "canonical-message-oracle" "$WORK/message-validate.out" || { echo "FAIL: Prnsd did not emit the stock embedded message"; exit 1; }

$BIN id -m "$PUBLIC" -e "$PLAINTEXT" > "$WORK/encrypt.out"
$PYTHON "$ORACLE" decrypt "$PRIVATE" "$PLAINTEXT.rfe" "$PLAINTEXT"
$BIN id -i "$PRIVATE" -d "$STOCK_ENCRYPTED" -w "$WORK/opened" > "$WORK/decrypt.out"
cmp "$PLAINTEXT" "$WORK/opened"

printf 'existing' > "$WORK/no-clobber"
set +e
NO_CLOBBER="$($BIN id -i "$PRIVATE" -d "$STOCK_ENCRYPTED" -w "$WORK/no-clobber" 2>&1)"
NO_CLOBBER_STATUS=$?
set -e
[ "$NO_CLOBBER_STATUS" -eq 11 ] || { echo "FAIL: no-clobber returned $NO_CLOBBER_STATUS"; echo "$NO_CLOBBER"; exit 1; }
[ "$(cat "$WORK/no-clobber")" = "existing" ] || { echo "FAIL: no-clobber changed the destination"; exit 1; }

cp "$STOCK_ENCRYPTED" "$WORK/corrupt.rfe"
$PYTHON -c 'import pathlib, sys; path = pathlib.Path(sys.argv[1]); data = bytearray(path.read_bytes()); data[64] ^= 1; path.write_bytes(data)' "$WORK/corrupt.rfe"
set +e
CORRUPT="$($BIN id -i "$PRIVATE" -d "$WORK/corrupt.rfe" -w "$WORK/partial" 2>&1)"
CORRUPT_STATUS=$?
set -e
[ "$CORRUPT_STATUS" -eq 12 ] || { echo "FAIL: corrupt decrypt returned $CORRUPT_STATUS"; echo "$CORRUPT"; exit 1; }
[ ! -e "$WORK/partial" ] || { echo "FAIL: corrupt decrypt published partial plaintext"; exit 1; }

for ENCODING in base32 base64 base256 hex; do
    case "$ENCODING" in
        base32) FLAG=-B ;;
        base64) FLAG=-b ;;
        base256) FLAG=-U ;;
        hex) FLAG=-F ;;
    esac
    EXPECTED="$($PYTHON "$ORACLE" encoding "$PUBLIC" "$ENCODING")"
    ACTUAL="$($BIN id -m "$PUBLIC" -x "$FLAG")"
    [[ "$ACTUAL" == *"$EXPECTED"* ]] || { echo "FAIL: $ENCODING identity output differs from stock RNS"; exit 1; }
done

$BIN id -i "$PRIVATE" -e "$PLAINTEXT" -O > "$WORK/stdout.rfe" 2> "$WORK/stdout.err"
$PYTHON "$ORACLE" decrypt "$PRIVATE" "$WORK/stdout.rfe" "$PLAINTEXT"
$BIN id -M "$($PYTHON -c 'import pathlib, sys; print(pathlib.Path(sys.argv[1]).read_bytes().hex())' "$PRIVATE")" -s --raw -I -O < "$CANDIDATE_MESSAGE" > "$WORK/stdin.rsg"
$PYTHON "$ORACLE" verify "$PRIVATE" "$CANDIDATE_MESSAGE" "$WORK/stdin.rsg"

$BIN id -g "$WORK/generated.rid" > "$WORK/generate.out"
[ "$(wc -c < "$WORK/generated.rid" | tr -d ' ')" -eq 64 ] || { echo "FAIL: generated identity has the wrong length"; exit 1; }

echo "PASS: Prnsd id matches stock RNS 1.4.2 identity, RSG/RSM, encryption, and encoding behavior"
