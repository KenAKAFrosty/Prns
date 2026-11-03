import { Alert, Platform } from "react-native";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { ContactDetailScreen } from "../src/screens/ContactDetailScreen";
import { saveContact, removeContact, setContactPinned } from "../src/contacts";

jest.mock("expo-router", () => ({ useLocalSearchParams: () => ({ owner: "owner", contact: "contact" }), useRouter: () => ({ replace: mockReplace, push: mockPush }) }));
jest.mock("../src/identity-reference", () => ({ identityKey: () => "owner" }));
jest.mock("../src/navigation", () => ({ scalar: (value: unknown) => typeof value === "string" ? value : undefined, useRouteActive: () => mockActive }));
jest.mock("../src/runtime/InstallationProvider", () => ({ useInstallation: () => ({ records: [mockRecord], contactSaved: mockChanged }) }));
jest.mock("../src/contacts", () => ({ contactKey: () => "contact", loadContacts: async () => [mockContact], saveContact: jest.fn(), removeContact: jest.fn(), setContactPinned: jest.fn() }));
jest.mock("../src/view-state", () => ({ useViewState: () => ({ value: { alias: "Edited" }, ready: true, change: jest.fn() }) }));
const mockRecord = { services: [0x67727073] };
const mockContact = { uri: "verified", alias: "Alex", claims: { displayName: "Alex", identityHash: new Uint8Array(16) } };
const mockPush = jest.fn(); const mockChanged = jest.fn(); const mockReplace = jest.fn(); let mockActive = true;
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }
beforeEach(() => { jest.clearAllMocks(); mockActive = true; });
async function option(title: string) {
  await act(async () => await fireEvent.press(screen.getByRole("button", { name: "Contact options" })));
  const dismiss = screen.container.queryAll((value) => typeof value.props.onDismiss === "function")[0]!.props.onDismiss;
  await act(async () => await fireEvent.press(screen.getByRole("button", { name: title })));
  if (Platform.OS === "ios") await act(async () => dismiss());
}

test("committed rename after leaving detail invalidates sibling contacts without navigating", async () => {
  const saved = deferred<Awaited<ReturnType<typeof saveContact>>>();
  jest.mocked(saveContact).mockReturnValue(saved.promise);
  const view = await render(<ContactDetailScreen />);
  await option("Edit private name");
  await act(async () => await fireEvent.press(screen.getByText("Save private name")));
  mockActive = false; await view.rerender(<ContactDetailScreen />);
  await act(async () => saved.resolve([]));
  expect(mockChanged).toHaveBeenCalledTimes(1);
  expect(mockReplace).not.toHaveBeenCalled();
  expect(saveContact).toHaveBeenCalledWith(mockRecord, "verified", "Edited");
});

test("committed removal after leaving detail invalidates sibling contacts but cannot redirect the current route", async () => {
  const removed = deferred<Awaited<ReturnType<typeof removeContact>>>(); jest.mocked(removeContact).mockReturnValue(removed.promise);
  const alert = jest.spyOn(Alert, "alert").mockImplementation(() => {});
  const view = await render(<ContactDetailScreen />);
  await option("Remove saved contact");
  await act(async () => alert.mock.calls[0]![2]![1]!.onPress!());
  mockActive = false; await view.rerender(<ContactDetailScreen />);
  await act(async () => removed.resolve([]));
  expect(mockChanged).toHaveBeenCalledTimes(1);
  expect(mockReplace).not.toHaveBeenCalled();
  expect(removeContact).toHaveBeenCalledWith(mockRecord, mockContact);
  alert.mockRestore();
});


test("pin is published only after durable success; failure leaves the contact and action available", async () => {
  jest.mocked(setContactPinned).mockRejectedValueOnce(new Error("Disk full"));
  await render(<ContactDetailScreen />);
  await option("Pin contact");
  expect(screen.getByText("Error: Disk full")).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "Contact options" }));
  expect(screen.getByRole("button", { name: "Pin contact" })).toBeTruthy();
  expect(mockChanged).not.toHaveBeenCalled();
  jest.mocked(setContactPinned).mockResolvedValue([{ ...mockContact, pinned: true }] as never);
  const dismiss = screen.container.queryAll((value) => typeof value.props.onDismiss === "function")[0]!.props.onDismiss;
  await act(async () => await fireEvent.press(screen.getByText("Pin contact")));
  if (Platform.OS === "ios") await act(async () => dismiss());
  await fireEvent.press(screen.getByRole("button", { name: "Contact options" }));
  expect(screen.getByRole("button", { name: "Unpin contact" })).toBeTruthy();
  expect(mockChanged).toHaveBeenCalledTimes(1);
  expect(setContactPinned).toHaveBeenLastCalledWith(mockRecord, mockContact, true);
});


test("common contact action is direct while options are deliberate and dismissible", async () => {
  await render(<ContactDetailScreen />);
  expect(screen.queryByText("Remove saved contact")).toBeNull();
  expect(screen.queryByLabelText("Private contact name")).toBeNull();
  await fireEvent.press(screen.getByRole("button", { name: "New conversation" }));
  expect(mockPush).toHaveBeenCalledWith({ pathname: "/conversations/new", params: { owner: "owner", contact: "contact" } });
  await fireEvent.press(screen.getByRole("button", { name: "Contact options" }));
  await fireEvent.press(screen.getByRole("button", { name: "Close Contact options" }));
  expect(removeContact).not.toHaveBeenCalled(); expect(setContactPinned).not.toHaveBeenCalled(); expect(saveContact).not.toHaveBeenCalled();
});
