import { StyleSheet } from "react-native";
import { fireEvent, render, screen } from "@testing-library/react-native";
import { InfoDisclosure } from "../src/ui/InfoDisclosure";
import { Heading, Text } from "../src/ui/primitives";

jest.mock("react-native-safe-area-context", () => {
  const { View } = require("react-native");
  return { SafeAreaView: View };
});

test("optional information stays out of the page until requested, while important results remain visible", async () => {
  await render(<><Heading help={{ label: "About contacts", content: <Text>Share a contact card to connect.</Text> }}>Contacts</Heading><Text accessibilityRole="alert">The contact was not saved. Try again.</Text></>);
  expect(screen.queryByText("Share a contact card to connect.")).toBeNull();
  expect(screen.getByRole("alert")).toHaveTextContent("The contact was not saved. Try again.");
  const button = screen.getByRole("button", { name: "About contacts" });
  expect(button.props.accessibilityState).toMatchObject({ expanded: false });
  expect(StyleSheet.flatten(button.props.style)).toMatchObject({ minWidth: 44, minHeight: 44 });
  await fireEvent.press(button);
  expect(screen.getByText("Share a contact card to connect.")).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "Close About contacts" }));
  expect(screen.queryByText("Share a contact card to connect.")).toBeNull();
  expect(screen.getByRole("alert")).toHaveTextContent("The contact was not saved. Try again.");
});

test("system back and outside tap dismiss the information sheet", async () => {
  await render(<InfoDisclosure label="About nodes"><Text>A radio board is optional.</Text></InfoDisclosure>);
  await fireEvent.press(screen.getByRole("button", { name: "About nodes" }));
  await fireEvent(screen.container.queryAll((value) => typeof value.props.onRequestClose === "function")[0]!, "requestClose");
  expect(screen.queryByText("A radio board is optional.")).toBeNull();
  await fireEvent.press(screen.getByRole("button", { name: "About nodes" }));
  const backdrop = screen.container.queryAll((item) => item.props.accessible === false && typeof item.props.onClick === "function")[0]!;
  await fireEvent.press(backdrop);
  expect(screen.queryByText("A radio board is optional.")).toBeNull();
});
