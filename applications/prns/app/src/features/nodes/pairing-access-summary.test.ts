import { RemoteControlRequestKind } from "@prns-internal/expo";

import { summarizePairingAccess } from "./pairing-access-summary";

describe("Pairing access summary", () => {
  it("does not invent access for an empty permission set", () => {
    expect(summarizePairingAccess([])).toBe("No node controls requested.");
  });

  it("keeps a read-only subset read-only", () => {
    expect(summarizePairingAccess([RemoteControlRequestKind.Describe])).toBe(
      "View node information.",
    );
  });

  it("keeps address sharing separate from reading and changing settings", () => {
    expect(summarizePairingAccess([RemoteControlRequestKind.AnnounceSelf])).toBe(
      "Share the node address.",
    );
  });

  it("does not describe inspecting controllers as granting or removing access", () => {
    expect(summarizePairingAccess([RemoteControlRequestKind.InventoryControllers])).toBe(
      "View node information.",
    );
  });

  it("discloses access management without inventing node-setting permissions", () => {
    expect(
      summarizePairingAccess([
        RemoteControlRequestKind.AuthorizeController,
        RemoteControlRequestKind.RevokeController,
      ]),
    ).toBe("Manage other devices’ access.");
  });

  it("does not imply read access for a write-only subset", () => {
    expect(summarizePairingAccess([RemoteControlRequestKind.SetDisplayVisibility])).toBe(
      "Change node settings.",
    );
  });

  it("summarizes all supported permissions in a stable, deduplicated order", () => {
    const permissions = Object.values(RemoteControlRequestKind).filter(
      (value): value is RemoteControlRequestKind => typeof value === "number",
    );
    expect(permissions).toHaveLength(30);
    const expected =
      "View node information. Change node settings. Share the node address. Manage other devices’ access.";
    expect(summarizePairingAccess(permissions)).toBe(expected);
    expect(summarizePairingAccess([...permissions].reverse())).toBe(expected);
    expect(summarizePairingAccess([...permissions, ...permissions])).toBe(expected);
    expect(expected).not.toMatch(/Full control/iu);
  });
});
