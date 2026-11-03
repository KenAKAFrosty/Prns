import { act, fireEvent, render, screen } from "@testing-library/react-native";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { Identities } from "../src/Identities";

jest.mock("@prns/app-bindings", () => ({ prns_identities_core: { IdentityDesiredState: { 0: "Paused" } } }));
jest.mock("../src/IdentityPanel", () => ({ IdentityPanel: () => null }));
jest.mock("../src/CatalogActivity", () => ({ CatalogActivity: () => null }));
jest.mock("../src/identity-creation", () => ({
  loadCreationDraft: async () => undefined, hex: (value: ArrayBuffer) => Buffer.from(value).toString("hex"),
  prepareCreationDraft: jest.fn(), submitCreationDraft: jest.fn(), CreationDraftConflict: class extends Error {}, CreationMetadataRejected: class extends Error {},
}));
const row = (label: string) => ({ id: { incarnation: new Uint8Array([1]).buffer, sequence: 1n }, publicHash: new Uint8Array([2]).buffer, desired: 0,
  metadata: { localLabel: label, publishedName: label } });

test("returning to identities rereads the catalog while preserving an unsubmitted name", async () => {
  const read = jest.fn().mockResolvedValue([row("Before")]);
  const owner = { identities: () => ({ identities: read }) } as unknown as owners.ServiceRuntimeLike;
  const props = { owner, onSelect: jest.fn() };
  const view = await render(<Identities {...props} active />);
  await fireEvent.changeText(screen.getByLabelText("Private identity label"), "Unsent identity");
  expect(screen.queryByText("Refresh identities")).toBeNull();
  await view.rerender(<Identities {...props} active={false} />);
  read.mockResolvedValue([row("After")]);
  await view.rerender(<Identities {...props} active />);
  expect(screen.getByText("Open After")).toBeTruthy(); expect(screen.getByDisplayValue("Unsent identity")).toBeTruthy();
});

test("a failed identity read has explicit retry without admitting a create", async () => {
  const read = jest.fn().mockRejectedValueOnce(new Error("Unavailable")).mockResolvedValue([]);
  const owner = { identities: () => ({ identities: read }) } as unknown as owners.ServiceRuntimeLike;
  await render(<Identities owner={owner} onSelect={jest.fn()} active />);
  await fireEvent.press(screen.getByRole("button", { name: "Retry loading identities" }));
  await act(async () => {});
  expect(screen.getByText(/No identities yet/)).toBeTruthy(); expect(screen.queryByText("Retry loading identities")).toBeNull();
});
