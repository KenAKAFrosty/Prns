import * as Native from "react-native";
import type { TestInstance } from "test-renderer";
import { fireEvent, renderRouter, screen, cleanup, act } from "expo-router/testing-library";
import { Shell } from "../src/ui/Shell";
import { Link } from "expo-router";
import { ListRow } from "../src/ui/primitives";

jest.mock("../src/runtime/identity-activity", () => ({ activityLabel: () => undefined }));
jest.mock("../src/runtime/IncomingContacts", () => ({ IncomingContacts: () => null }));
jest.mock("../src/runtime/InstallationProvider", () => ({ useInstallation: () => ({
  records: [], selected: "selected-identity", activity: {}, select: jest.fn(),
}) }));
jest.mock("react-native-safe-area-context", () => {
  const { View } = require("react-native");
  return { SafeAreaProvider: View, SafeAreaView: View };
});

async function open(width: number) {
  jest.spyOn(require("react-native"), "useWindowDimensions").mockReturnValue({ width, height: 800, scale: 2, fontScale: 1.3 });
  const view = await renderRouter({
    _layout: Shell,
    conversations: () => <Native.Text>Saved conversations</Native.Text>,
    contacts: () => <Native.Text>Saved contacts</Native.Text>,
    nodes: () => <Native.Text>Saved nodes</Native.Text>,
    more: () => <Native.Text>More settings</Native.Text>,
  }, { initialUrl: "/conversations" });
  await act(async () => {});
  return view;
}
afterEach(async () => { await cleanup(); jest.restoreAllMocks(); });

function itemBox(icon: TestInstance) {
  let node = icon.parent;
  while (node !== null && Native.StyleSheet.flatten(node.props.style)?.minHeight === undefined) node = node.parent;
  if (node === null) throw new Error("The rendered navigation item lost its layout through Link");
  return node;
}

test("all compact tab icons stay centered and top-aligned through the real Link child, including wrapping labels", async () => {
  const view = await open(320);
  for (const label of ["Conversations", "Contacts", "Nodes", "More"]) {
    const tab = screen.getByRole("link", { name: label });
    const slots = tab.queryAll((node) => {
      const style = Native.StyleSheet.flatten(node.props.style);
      return style?.width === 24 && style?.height === 24;
    });
    expect(slots).toHaveLength(1);
    const slot = slots[0]!;
    const content = itemBox(slot);
    expect(Native.StyleSheet.flatten(slot.props.style)).toMatchObject({ width: 24, height: 24, alignItems: "center" });
    const itemStyle = Native.StyleSheet.flatten(content.props.style);
    expect(itemStyle).toMatchObject({ alignItems: "center", justifyContent: "flex-start", flexGrow: 1, minHeight: 52, paddingVertical: 4 });
    expect(itemStyle.flex).toBeUndefined();
    expect(itemStyle.flexBasis).toBeUndefined();
  }
  for (const label of ["Conversations", "Contacts", "Nodes", "More"]) {
    const tab = screen.getByRole("link", { name: label });
    const pressableStyle = Native.StyleSheet.flatten(tab.props.style);
    expect(pressableStyle).toMatchObject({ flexGrow: 1, width: "100%" });
    expect(pressableStyle.flex).toBeUndefined();
    expect(pressableStyle.flexBasis).toBeUndefined();
    expect(screen.getByText(label).props.numberOfLines).toBeUndefined();
  }
  expect(screen.getByRole("link", { name: "Conversations" }).props.accessibilityState).toMatchObject({ selected: true });
  await fireEvent.press(screen.getByRole("link", { name: "Contacts" }));
  expect(await screen.findByText("Saved contacts")).toBeTruthy();
  expect(view.getSearchParams()).toMatchObject({ owner: "selected-identity" });
  expect(screen.getByRole("link", { name: "Contacts" }).props.accessibilityState).toMatchObject({ selected: true });
});

test("wide navigation keeps icons beside labels and retains visible press and focus feedback", async () => {
  await open(1000);
  const label = screen.getByText("Contacts");
  const content = itemBox(label);
  expect(Native.StyleSheet.flatten(content.props.style)).toMatchObject({ flexDirection: "row", alignItems: "center" });
  const idle = Native.StyleSheet.flatten(content.props.style);
  await fireEvent(screen.getByRole("link", { name: "Contacts" }), "pressIn");
  expect(Native.StyleSheet.flatten(content.props.style).backgroundColor).not.toBe(idle.backgroundColor);
  await fireEvent(screen.getByRole("link", { name: "Contacts" }), "focus");
  expect(Native.StyleSheet.flatten(content.props.style).borderColor).not.toBe("transparent");
  await fireEvent(screen.getByRole("link", { name: "Contacts" }), "blur");
  expect(Native.StyleSheet.flatten(content.props.style).backgroundColor).toBe(idle.backgroundColor);
});

test("the header info button opens context and closes without navigating or changing the identity", async () => {
  const view = await open(390);
  expect(screen.queryByText("Choose a messaging identity to keep its contacts and conversations separate.")).toBeNull();
  await fireEvent.press(screen.getByRole("button", { name: "About PRNS Chat" }));
  expect(screen.getByText("Choose a messaging identity to keep its contacts and conversations separate.")).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "Close About PRNS Chat" }));
  expect(screen.queryByText("Choose a messaging identity to keep its contacts and conversations separate.")).toBeNull();
  expect(view.getPathname()).toBe("/conversations");
});


test("a list row forwards real Link navigation and accessibility without a fake press handler", async () => {
  const view = await renderRouter({
    index: () => <Link href={{ pathname: "/detail", params: { owner: "captured-owner" } }} asChild><ListRow title="Connection settings" subtitle="Saved network choices" /></Link>,
    detail: () => <Native.Text>Connection detail</Native.Text>,
  }, { initialUrl: "/" });
  await act(async () => {});
  await fireEvent.press(screen.getByRole("link", { name: "Connection settings" }));
  expect(await screen.findByText("Connection detail")).toBeTruthy();
  expect(view.getSearchParams()).toMatchObject({ owner: "captured-owner" });
});
