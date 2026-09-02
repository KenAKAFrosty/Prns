import { navigationEntries, screenCatalog } from "./catalog";

describe("screen catalog", () => {
  it("keeps one closed, uniquely addressed route inventory", () => {
    expect(screenCatalog).toHaveLength(33);
    expect(new Set(screenCatalog.map(({ id }) => id)).size).toBe(33);
    expect(new Set(screenCatalog.map(({ path }) => path)).size).toBe(33);

    const routeFiles = screenCatalog.flatMap(({ routeFiles: files }) => files);
    expect(routeFiles).toHaveLength(34);
    expect(new Set(routeFiles).size).toBe(34);
  });

  it("marks only the eight honest presentation slices as implemented", () => {
    expect(
      screenCatalog
        .filter(({ availability }) => availability === "implementedScaffold")
        .map(({ id }) => id),
    ).toEqual([
      "installation.onboarding",
      "nodes.index",
      "nodes.managed",
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
      "nodes.index",
      "explore.index",
      "more.index",
    ]);
  });
});
