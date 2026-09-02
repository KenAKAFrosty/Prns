export const developmentFingerprint =
  "Development fixture — not a Reticulum identity or destination";

export const identityFixtures = [
  {
    id: "identity.generated-preview",
    label: "Generated identity preview",
    fingerprint: developmentFingerprint,
  },
  {
    id: "identity.imported-preview",
    label: "Imported identity preview",
    fingerprint: developmentFingerprint,
  },
] as const;

export type IdentityFixtureId = (typeof identityFixtures)[number]["id"];

export const managedNodeFixtures = [
  {
    id: "node.e290-preview",
    label: "E290 managed-node preview",
    fingerprint: developmentFingerprint,
  },
] as const;

export type ManagedNodeFixtureId = (typeof managedNodeFixtures)[number]["id"];

export function identityFixture(id: IdentityFixtureId | null) {
  return identityFixtures.find((fixture) => fixture.id === id);
}

export function managedNodeFixture(id: ManagedNodeFixtureId | null) {
  return managedNodeFixtures.find((fixture) => fixture.id === id);
}
