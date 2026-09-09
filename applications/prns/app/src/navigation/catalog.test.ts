import { navigationEntries, screenById, screenCatalog } from "./catalog";

describe("screen catalog", () => {
  it("keeps one closed, uniquely addressed route inventory", () => {
    expect(screenCatalog).toHaveLength(33);
    expect(new Set(screenCatalog.map(({ id }) => id)).size).toBe(33);
    expect(new Set(screenCatalog.map(({ path }) => path)).size).toBe(33);

    const routeFiles = screenCatalog.flatMap(({ routeFiles: files }) => files);
    expect(routeFiles).toHaveLength(34);
    expect(new Set(routeFiles).size).toBe(34);
  });

  it("marks only the seventeen honest presentation slices as implemented", () => {
    expect(
      screenCatalog
        .filter(({ availability }) => availability === "implementedScaffold")
        .map(({ id }) => id),
    ).toEqual([
      "installation.onboarding",
      "installation.recovery",
      "inbox.index",
      "inbox.conversation",
      "inbox.compose",
      "contacts.index",
      "contacts.entry",
      "contacts.add",
      "nodes.index",
      "nodes.local",
      "nodes.managed",
      "nodes.pair",
      "explore.index",
      "more.index",
      "settings.index",
      "settings.about",
      "help.index",
    ]);
  });

  it("orders navigation roots and can hide unavailable entries", () => {
    expect(navigationEntries("phone", true).map(({ id }) => id)).toEqual([
      "inbox.index",
      "contacts.index",
      "nodes.index",
      "explore.index",
      "more.index",
    ]);
    expect(navigationEntries("phone", false).map(({ id }) => id)).toEqual([
      "inbox.index",
      "contacts.index",
      "nodes.index",
      "explore.index",
      "more.index",
    ]);
  });

  it("distinguishes implemented native state from remaining placeholders", () => {
    expect(screenById("more.index")).toMatchObject({
      summary: "Open identities, connections, notifications, settings, and help.",
      limitation: "Some areas are still being built.",
    });
    expect(screenById("notifications.settings").limitation).toBe(
      "Notifications and background message delivery are not available yet.",
    );
    expect(screenById("settings.storage").limitation).toBe(
      "Storage details are not available yet.",
    );
    expect(screenById("help.index").summary).toBe(
      "See what works in this preview and what is coming later.",
    );
  });

  it.each([
    [
      "inbox.index",
      "Messaging is available on iOS and Android. Background delivery is not guaranteed.",
    ],
    ["contacts.index", "Contacts are available on iOS and Android."],
    ["nodes.index", "Node management is available on iOS and Android."],
    ["nodes.local", "Detailed node status is available on iOS and Android."],
  ] as const)("describes the supported mobile platforms for %s", (id, limitation) => {
    expect(screenById(id).limitation).toBe(limitation);
  });
});
