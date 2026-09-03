import { formatBytes, formatRequestKind, formatRuntime } from "./format";

describe("Nodes presentation formatting", () => {
  it("renders exact byte values without inventing a text identity", () => {
    expect(formatBytes(Uint8Array.from([0, 15, 16, 255]))).toBe("000f10ff");
  });

  it("labels generated closed values exhaustively", () => {
    expect(formatRequestKind("announceSelf")).toBe("Share node address");
    expect(formatRequestKind("describe")).toBe("View node information");
    expect(formatRuntime("stopping")).toBe("Stopping");
  });
});
