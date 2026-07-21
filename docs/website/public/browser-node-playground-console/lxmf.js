import { Tag, appData, appName, aspect, } from "./sdk/index.js";
const MESSAGEPACK_FIXARRAY_TWO = 0x92;
const MESSAGEPACK_BIN8 = 0xc4;
const MESSAGEPACK_NIL = 0xc0;
const MESSAGEPACK_BIN8_MAX_LENGTH = 255;
const MESSAGEPACK_BIN8_HEADER_LENGTH = 3;
const MESSAGEPACK_NIL_LENGTH = 1;
export const LXMF_DELIVERY_DISPLAY_NAME = "Prns Browser Playground";
export const BROWSER_PLAYGROUND_LXMF_DELIVERY = prepareLxmfDeliveryProfile(LXMF_DELIVERY_DISPLAY_NAME);
function prepareLxmfDeliveryProfile(displayName) {
    const displayNameBytes = new TextEncoder().encode(displayName);
    if (displayNameBytes.length > MESSAGEPACK_BIN8_MAX_LENGTH) {
        return Tag("LxmfDisplayNameTooLong", {
            actual: displayNameBytes.length,
            maximum: MESSAGEPACK_BIN8_MAX_LENGTH,
        });
    }
    const encoded = new Uint8Array(MESSAGEPACK_BIN8_HEADER_LENGTH +
        displayNameBytes.length +
        MESSAGEPACK_NIL_LENGTH);
    encoded[0] = MESSAGEPACK_FIXARRAY_TWO;
    encoded[1] = MESSAGEPACK_BIN8;
    encoded[2] = displayNameBytes.length;
    encoded.set(displayNameBytes, MESSAGEPACK_BIN8_HEADER_LENGTH);
    encoded[encoded.length - 1] = MESSAGEPACK_NIL;
    return Tag("Prepared", {
        displayName,
        registration: {
            appName: appName("lxmf"),
            aspects: [aspect("delivery")],
            appData: appData(encoded),
        },
    });
}
