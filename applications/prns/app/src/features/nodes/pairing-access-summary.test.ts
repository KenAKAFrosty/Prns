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

  it("describes application messages without implying settings or access management", () => {
    expect(summarizePairingAccess([RemoteControlRequestKind.AppMessage])).toBe(
      "Send application messages.",
    );
  });

  it("keeps interface watches and radio inspection in the read-only category", () => {
    expect(
      summarizePairingAccess([
        RemoteControlRequestKind.WatchInterfaces,
        RemoteControlRequestKind.DescribeNodeName,
        RemoteControlRequestKind.InspectRadio,
      ]),
    ).toBe("View node information.");
  });

  it("describes node naming and radio configuration as settings changes", () => {
    expect(
      summarizePairingAccess([
        RemoteControlRequestKind.SetNodeName,
        RemoteControlRequestKind.ConfigureRadio,
      ]),
    ).toBe("Change node settings.");
  });

  it("summarizes all supported permissions in a stable, deduplicated order", () => {
    const permissions = Object.values(RemoteControlRequestKind).filter(
      (value): value is RemoteControlRequestKind => typeof value === "number",
    );
    expect(permissions).toHaveLength(36);
    const expected =
      "View node information. Change node settings. Share the node address. Send application messages. Manage other devices’ access.";
    expect(summarizePairingAccess(permissions)).toBe(expected);
    expect(summarizePairingAccess([...permissions].reverse())).toBe(expected);
    expect(summarizePairingAccess([...permissions, ...permissions])).toBe(expected);
    expect(expected).not.toMatch(/Full control/iu);
  });
});
