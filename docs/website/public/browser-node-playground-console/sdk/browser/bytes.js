export function byteKey(bytes) {
    let key = "";
    for (const byte of bytes) {
        key += byte.toString(16).padStart(2, "0");
    }
    return key;
}
