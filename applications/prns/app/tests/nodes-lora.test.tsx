import { act, fireEvent, render } from "@testing-library/react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { integer, LoRa } from "../src/nodes/LoRa";

jest.mock("@prns/app-bindings", () => {
  const tags = new Proxy({}, { get: (_, key) => key });
  const variants = new Proxy({}, { get: (_, key) => class { tag = key; inner: unknown; constructor(value?: unknown) { this.inner = value; } } });
  const enumeration = (...names: string[]) => Object.fromEntries(names.flatMap((name, index) => [[name, index], [index, name]]));
  return { prns_host_uniffi: {
    RemoteControlRequest: variants, RemoteControlModulation: variants, RemoteControlBandProfile: variants, RemoteControlSubGRegion: variants, RemoteControlRadioConfiguration: variants,
    RemoteControlRadioStatus_Tags: tags, RemoteControlBandProfile_Tags: tags, RemoteControlSubGRegion_Tags: tags,
    RemoteControlRadioProfileSettlement_Tags: tags, RemoteControlGhz24ProfileSettlement_Tags: tags,
    RemoteControlRequestKind: enumeration("SetInterfaceLoRaProfile", "ConfigureRadio"), RemoteControlRadioBands: enumeration("SubG", "SubGAndGhz24"),
    RemoteControlRegulatoryRegion: enumeration("Us915", "Eu868"), RemoteControlSpreadingFactor: enumeration("Sf7"),
    RemoteControlLoraBandwidth: enumeration("Bw125kHz"), RemoteControlCodingRate: enumeration("Cr45"),
    remoteControlLoraProfileBand: jest.fn(), remoteControlRadioProfile: jest.fn(), remoteControlGhz24Profile: jest.fn(),
    remoteControlLoraProfileFromBand: jest.fn(() => ({ value: "canonical-native-encoding" })),
  }};
});
async function fill(screen: Awaited<ReturnType<typeof render>>) {
  await fireEvent.changeText(screen.getByLabelText("Radio frequency in Hz"), "915000000");
  await fireEvent.changeText(screen.getByLabelText("Transmit power in dBm"), "10");
  await fireEvent.changeText(screen.getByLabelText("Preamble symbols"), "8");
  for (const name of ["Region: Us915", "Spreading factor: Sf7", "Bandwidth: 125 kHz", "Coding rate: 4/5"]) await act(async () => await fireEvent.press(screen.getByLabelText(name)));
}

test("numeric radio editor delegates validation and wire formatting to core helpers", async () => {
  const review = jest.fn(); const id = { value: new Uint8Array(16).fill(3) };
  (rc.remoteControlRadioProfile as jest.Mock).mockReturnValue({ tag: "Completed", inner: { value: { canonicalProfile: true } } });
  const screen = await render(<LoRa id={id} radio={undefined} reported={undefined} supported={{ kinds: [0] }} disabled={false} review={review} />);
  expect(screen.queryByText("2.4 GHz")).toBeNull();
  await fill(screen);
  await act(async () => await fireEvent.press(screen.getByText("Validate and review LoRa profile")));
  expect(rc.remoteControlRadioProfile).toHaveBeenCalledWith(expect.objectContaining({ tag: "Regulated", inner: { value: 0 } }), { value: 915000000 }, expect.objectContaining({ tag: "Lora", inner: { spreadingFactor: 0, bandwidth: 0, codingRate: 0 } }), { value: 10 }, { value: 8 });
  expect(rc.remoteControlLoraProfileFromBand).toHaveBeenCalledWith(expect.objectContaining({ tag: "SubG", inner: { value: { canonicalProfile: true } } }));
  expect(review).toHaveBeenCalledWith(expect.objectContaining({ tag: "SetInterfaceLoRaProfile", inner: { id, profile: { value: "canonical-native-encoding" } } }));
});

test("regional/profile rejection prevents any remote review or mutation", async () => {
  const review = jest.fn();
  (rc.remoteControlRadioProfile as jest.Mock).mockReturnValue({ tag: "Failed", inner: { failure: { tag: "TransmitPowerAboveRegionLimit" } } });
  const screen = await render(<LoRa id={{ value: new Uint8Array(16) }} radio={undefined} reported={undefined} supported={{ kinds: [0] }} disabled={false} review={review} />);
  await fill(screen);
  await act(async () => await fireEvent.press(screen.getByText("Validate and review LoRa profile")));
  expect(review).not.toHaveBeenCalled(); expect(screen.getByText("Transmit Power Above Region Limit")).toBeTruthy();
});

test("numeric input never rounds or truncates a value crossing the native ABI", () => {
  expect(integer("915000000", 0, 0xffff_ffff)).toBe(915000000);
  expect(integer("4294967296", 0, 0xffff_ffff)).toBeUndefined();
  expect(integer("9007199254740993", 0, Number.MAX_SAFE_INTEGER)).toBeUndefined();
  expect(integer("1.5", 0, 65535)).toBeUndefined();
  expect(integer("1e3", 0, 65535)).toBeUndefined();
});
