import * as Bindings from "@prns-internal/expo";
import { formatBytes, formatControllerAuthority, formatRequestKind, formatRuntime } from "./format";
describe("Nodes presentation formatting", () => {
  it("renders exact byte values without inventing a text identity", () => {
    expect(formatBytes(Uint8Array.from([0, 15, 16, 255]))).toBe("000f10ff");
  });
  it("labels generated closed values exhaustively", () => {
    expect(formatRequestKind(Bindings.RemoteControlRequestKind.AnnounceSelf)).toBe(
      "Share node address",
    );
    expect(formatRequestKind(Bindings.RemoteControlRequestKind.Describe)).toBe(
      "View node information",
    );
    expect(formatRuntime(Bindings.DevelopmentNodeRuntime.Stopping)).toBe("Stopping");
  });
  it("labels every supported permission and both access levels", () => {
    const kinds = Object.values(Bindings.RemoteControlRequestKind).filter(
      (value): value is Bindings.RemoteControlRequestKind => typeof value === "number",
    );
    expect(kinds).toHaveLength(30);
    for (const kind of kinds) expect(formatRequestKind(kind)).toMatch(/\S/);
    expect(formatControllerAuthority(Bindings.RemoteControlControllerAuthority.Operator)).toBe(
      "Operator",
    );
    expect(formatControllerAuthority(Bindings.RemoteControlControllerAuthority.Administrator)).toBe(
      "Administrator",
    );
  });
});
