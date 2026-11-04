import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import * as DocumentPicker from "expo-document-picker";
import { prns_chat_app as app } from "@prns/app-bindings";
import { ImportIdentityScreen } from "../src/screens/ImportIdentityScreen";

const mockDelete = jest.fn();
const mockInstallation = { mode: "create" as "create" | "open", busy: undefined as "start" | undefined,
  error: undefined as string | undefined, owner: undefined as object | undefined, bluetooth: true, startImported: jest.fn().mockResolvedValue(undefined) };
jest.mock("../src/runtime/InstallationProvider", () => ({ useInstallation: () => mockInstallation }));
jest.mock("../src/ui/primitives", () => {
  const { View, Text, Button } = require("react-native");
  return { Screen: View, Panel: View, Heading: Text, Text, Button };
});
jest.mock("expo-router", () => ({ Link: ({ children }: { children: unknown }) => children }));
jest.mock("expo-document-picker", () => ({ getDocumentAsync: jest.fn() }));
jest.mock("expo-file-system", () => ({ Paths: { cache: { uri: "file:///cache/" } },
  File: class { delete() { mockDelete(); } } }));
jest.mock("@prns/app-bindings", () => ({ prns_chat_app: {
  appPendingPhoneIdentityImport: jest.fn(), appReviewPhoneIdentityFile: jest.fn(),
  appRetainPhoneIdentityImport: jest.fn(), appCancelPhoneIdentityImport: jest.fn(),
} }));
const selection = { id: "a".repeat(64), identityHash: new Uint8Array(16).fill(0x2a).buffer };
const picked = { canceled: false as const, assets: [{ uri: "file:///cache/key%20copy", name: "key", size: 64, lastModified: 0 }] };
beforeEach(() => {
  jest.resetAllMocks();
  mockInstallation.mode = "create"; mockInstallation.busy = undefined; mockInstallation.error = undefined; mockInstallation.owner = undefined;
  mockInstallation.startImported.mockResolvedValue(undefined);
  jest.mocked(app.appPendingPhoneIdentityImport).mockReturnValue(undefined);
  jest.mocked(app.appRetainPhoneIdentityImport).mockReturnValue(selection);
  jest.mocked(DocumentPicker.getDocumentAsync).mockResolvedValue(picked);
});
afterEach(cleanup);

test("a retained review survives UI reload and never starts without confirmation", async () => {
  jest.mocked(app.appPendingPhoneIdentityImport).mockReturnValue(selection);
  await render(<ImportIdentityScreen />);
  expect(screen.getByText("2a".repeat(16))).toBeTruthy();
  expect(mockInstallation.startImported).not.toHaveBeenCalled();
  await act(async () => { await fireEvent.press(screen.getByText("Set up with this identity")); });
  expect(mockInstallation.startImported).toHaveBeenCalledWith(selection.id, true);
  expect(DocumentPicker.getDocumentAsync).not.toHaveBeenCalled();
});

test("the native review reads a local path and the picker copy is removed before retention", async () => {
  const order: string[] = [];
  jest.mocked(app.appReviewPhoneIdentityFile).mockImplementation(() => { order.push("review"); return {} as app.PhoneIdentityImportLike; });
  mockDelete.mockImplementation(() => order.push("delete"));
  jest.mocked(app.appRetainPhoneIdentityImport).mockImplementation(() => { order.push("retain"); return selection; });
  await render(<ImportIdentityScreen />);
  await act(async () => { await fireEvent.press(screen.getByText("Choose identity file")); });
  expect(DocumentPicker.getDocumentAsync).toHaveBeenCalledWith({ type: "*/*", multiple: false, copyToCacheDirectory: true });
  expect(app.appReviewPhoneIdentityFile).toHaveBeenCalledWith("/cache/key copy");
  expect(order).toEqual(["review", "delete", "retain"]);
  expect(mockInstallation.startImported).not.toHaveBeenCalled();
});

test("invalid keys still remove their picker copy and cannot replace the retained review", async () => {
  jest.mocked(app.appReviewPhoneIdentityFile).mockImplementation(() => { throw new Error("Invalid identity file"); });
  await render(<ImportIdentityScreen />);
  await act(async () => { await fireEvent.press(screen.getByText("Choose identity file")); });
  expect(screen.getByText("Invalid identity file")).toBeTruthy();
  expect(mockDelete).toHaveBeenCalledTimes(1);
  expect(app.appRetainPhoneIdentityImport).not.toHaveBeenCalled();
});

test("a picker result after leaving the screen is cleaned without retaining or importing it", async () => {
  let resolve!: (value: typeof picked) => void;
  jest.mocked(DocumentPicker.getDocumentAsync).mockReturnValue(new Promise((done) => { resolve = done; }));
  const view = await render(<ImportIdentityScreen />);
  await fireEvent.press(screen.getByText("Choose identity file"));
  await view.unmount();
  await act(async () => { resolve(picked); });
  expect(mockDelete).toHaveBeenCalledTimes(1);
  expect(app.appReviewPhoneIdentityFile).not.toHaveBeenCalled();
  expect(app.appRetainPhoneIdentityImport).not.toHaveBeenCalled();
});

test("a noncache URI is neither read nor deleted", async () => {
  jest.mocked(DocumentPicker.getDocumentAsync).mockResolvedValue({ ...picked, assets: [{ ...picked.assets[0]!, uri: "file:///documents/original" }] });
  await render(<ImportIdentityScreen />);
  await act(async () => { await fireEvent.press(screen.getByText("Choose identity file")); });
  expect(screen.getByText("The selected file was not copied into the private cache.")).toBeTruthy();
  expect(mockDelete).not.toHaveBeenCalled();
  expect(app.appReviewPhoneIdentityFile).not.toHaveBeenCalled();
});

test("a failed temporary deletion blocks import and offers an explicit cleanup retry", async () => {
  mockDelete.mockImplementation(() => { throw new Error("Cache removal failed"); });
  await render(<ImportIdentityScreen />);
  await act(async () => { await fireEvent.press(screen.getByText("Choose identity file")); });
  expect(app.appRetainPhoneIdentityImport).not.toHaveBeenCalled();
  expect(screen.getByText("Choose identity file")).toBeDisabled();
  mockDelete.mockImplementation(() => undefined);
  await act(async () => { await fireEvent.press(screen.getByText("Remove temporary copy")); });
  expect(screen.getByText("Choose identity file")).toBeEnabled();
});

test("a partial import remains visible without automatic replay and explains reset", async () => {
  jest.mocked(app.appPendingPhoneIdentityImport).mockReturnValue(selection);
  mockInstallation.mode = "open"; mockInstallation.error = "Local setup failed";
  await render(<ImportIdentityScreen />);
  expect(screen.getByText("Set up with this identity")).toBeDisabled();
  expect(screen.getByText(/setup failed partway/)).toBeTruthy();
  expect(mockInstallation.startImported).not.toHaveBeenCalled();
});

test("successful local setup shows continuation without the existing-installation warning", async () => {
  mockInstallation.mode = "open"; mockInstallation.owner = {};
  await render(<ImportIdentityScreen />);
  expect(screen.getByText("Your phone is ready. Continue to your messaging identities.")).toBeTruthy();
  expect(screen.getByText("Continue to messaging identities")).toBeTruthy();
  expect(screen.queryByText(/This phone is already set up/)).toBeNull();
  expect(screen.queryByText("Open saved installation")).toBeNull();
  expect(mockInstallation.startImported).not.toHaveBeenCalled();
});
