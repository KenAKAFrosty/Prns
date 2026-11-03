import { Platform, StyleSheet } from "react-native";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { Disclosure } from "../src/ui/Disclosure";
import { ActionMenu, Actions, Button, PageHeader, Text } from "../src/ui/primitives";

jest.mock("react-native-safe-area-context", () => { const { View } = require("react-native"); return { SafeAreaView: View }; });

test("compact actions keep their labels, 44pt targets and explicit full-width choice", async () => {
  const press = jest.fn();
  await render(<Actions><Button title="Create" tone="primary" icon="add" onPress={press} />
    <Button title="Continue" fill onPress={press} /></Actions>);
  expect(StyleSheet.flatten(screen.getByRole("button", { name: "Create" }).props.style)).toMatchObject({ alignSelf: "flex-start", minHeight: 44, maxWidth: "100%" });
  expect(StyleSheet.flatten(screen.getByRole("button", { name: "Continue" }).props.style)).toMatchObject({ alignSelf: "stretch", width: "100%" });
  await fireEvent.press(screen.getByRole("button", { name: "Create" })); expect(press).toHaveBeenCalledTimes(1);
});

test("options open without acting, dismiss cleanly, and run only the selected enabled action", async () => {
  const remove = jest.fn(); const edit = jest.fn();
  await render(<ActionMenu label="Options" items={[{ title: "Edit name", onPress: edit }, { title: "Remove", destructive: true, disabled: true, onPress: remove }]} />);
  expect(screen.queryByRole("button", { name: "Edit name" })).toBeNull();
  await fireEvent.press(screen.getByRole("button", { name: "Options" }));
  await fireEvent.press(screen.getByRole("button", { name: "Remove" })); expect(remove).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByRole("button", { name: "Close Options" })); expect(edit).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByRole("button", { name: "Options" }));
  const dismiss = screen.container.queryAll((value) => typeof value.props.onDismiss === "function")[0]!.props.onDismiss;
  await fireEvent.press(screen.getByRole("button", { name: "Edit name" }));
  if (Platform.OS === "ios") { expect(edit).not.toHaveBeenCalled(); await act(async () => dismiss()); }
  expect(edit).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("button", { name: "Edit name" })).toBeNull();
});

test("losing action availability closes an open menu without restoring it later", async () => {
  const run = jest.fn(); const items = [{ title: "Edit", onPress: run }];
  const view = await render(<ActionMenu label="Options" items={items} />);
  await fireEvent.press(screen.getByRole("button", { name: "Options" }));
  await view.rerender(<ActionMenu label="Options" items={items} disabled />);
  expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
  await view.rerender(<ActionMenu label="Options" items={items} />);
  expect(screen.queryByRole("button", { name: "Edit" })).toBeNull(); expect(run).not.toHaveBeenCalled();
});

test("page utility menus, contextual help and primary actions remain independent", async () => {
  const run = jest.fn(); const options = jest.fn();
  await render(<PageHeader title="Contacts" help={{ label: "About contacts", content: <Text>Context only</Text> }}
    trailing={<ActionMenu label="Contact options" items={[{ title: "View saved actions", onPress: options }]} />}>
    <Button title="Add contact" onPress={run} /></PageHeader>);
  await fireEvent.press(screen.getByRole("button", { name: "About contacts" })); expect(screen.getByText("Context only")).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "Close About contacts" })); expect(run).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByRole("button", { name: "Add contact" })); expect(run).toHaveBeenCalledTimes(1);
  await fireEvent.press(screen.getByRole("button", { name: "Contact options" }));
  expect(screen.getByRole("button", { name: "View saved actions" })).toBeTruthy();
  expect(options).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByRole("button", { name: "Close Contact options" }));
  expect(run).toHaveBeenCalledTimes(1);
});


test("an iOS action waiting for native dismissal cannot run after the menu is retired", async () => {
  jest.replaceProperty(Platform, "OS", "ios");
  const run = jest.fn(); const items = [{ title: "Remove", onPress: run }];
  const view = await render(<ActionMenu label="Options" items={items} />);
  await fireEvent.press(screen.getByRole("button", { name: "Options" }));
  const dismiss = screen.container.queryAll((value) => typeof value.props.onDismiss === "function")[0]!.props.onDismiss;
  await fireEvent.press(screen.getByRole("button", { name: "Remove" }));
  expect(run).not.toHaveBeenCalled();
  await view.rerender(<ActionMenu label="Options" items={items} disabled />);
  await act(async () => dismiss()); expect(run).not.toHaveBeenCalled();
  jest.restoreAllMocks();
});


test("a labeled disclosure exposes expansion state and reveals choices without running an action", async () => {
  const save = jest.fn();
  await render(<Disclosure label="Advanced choices"><Button title="Save choice" onPress={save} /></Disclosure>);
  expect(screen.getByText("Advanced choices")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Advanced choices" }).props.accessibilityState).toMatchObject({ expanded: false });
  expect(screen.queryByRole("button", { name: "Save choice" })).toBeNull();
  await fireEvent.press(screen.getByRole("button", { name: "Advanced choices" }));
  expect(screen.getByRole("button", { name: "Advanced choices" }).props.accessibilityState).toMatchObject({ expanded: true });
  expect(save).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByRole("button", { name: "Save choice" }));
  expect(save).toHaveBeenCalledTimes(1);
  await fireEvent.press(screen.getByRole("button", { name: "Advanced choices" }));
  expect(screen.getByRole("button", { name: "Advanced choices" }).props.accessibilityState).toMatchObject({ expanded: false });
  expect(screen.queryByRole("button", { name: "Save choice" })).toBeNull();
  expect(save).toHaveBeenCalledTimes(1);
});
