import { fireEvent, render, screen } from "@testing-library/react-native";
import { StyleSheet } from "react-native";
import { Paging } from "../../src/messaging/Paging";

function page() {
  return { busy: false, loaded: true, error: undefined as string | undefined, page: 1, hasNext: false, hasPrevious: false,
    next: jest.fn(), previous: jest.fn(), first: jest.fn(), retry: jest.fn() };
}

test("a loaded single page has no routine refresh, arrows, counter or loading indicator", async () => {
  const state = page();
  const view = await render(<Paging label="Conversations" active page={state} />);
  expect(view.toJSON()).toBeNull();
  await view.rerender(<Paging label="Conversations" active page={{ ...state, busy: true }} />);
  expect(view.toJSON()).toBeNull();
});

test("First remains reachable at the retained cursor window boundary even without previous or next", async () => {
  const state = { ...page(), page: 37 };
  await render(<Paging label="Conversations" active page={state} />);
  expect(screen.getByLabelText("Conversations page 37")).toHaveTextContent("Page 37");
  await fireEvent.press(screen.getByRole("button", { name: "First conversations page" }));
  expect(state.first).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: "Previous conversations" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Next conversations" })).toBeDisabled();
});

test("loaded-page navigation remains actionable and visually stable during background refresh", async () => {
  const state = { ...page(), hasNext: true, hasPrevious: true, page: 2 };
  const view = await render(<Paging label="Members" active page={state} />);
  const style = StyleSheet.flatten(screen.getByRole("button", { name: "Next members" }).props.style);
  await view.rerender(<Paging label="Members" active page={{ ...state, busy: true }} />);
  expect(screen.queryByLabelText("Loading members")).toBeNull();
  const next = screen.getByRole("button", { name: "Next members" });
  expect(next).not.toBeDisabled();
  expect(StyleSheet.flatten(next.props.style)).toEqual(style);
  expect(style).toMatchObject({ width: 44, height: 44 });
  await fireEvent.press(next);
  await fireEvent.press(screen.getByRole("button", { name: "Previous members" }));
  expect(state.next).toHaveBeenCalledTimes(1);
  expect(state.previous).toHaveBeenCalledTimes(1);
});

test("failed later pages offer Retry and First while previous/next remain fenced", async () => {
  const state = { ...page(), page: 5, loaded: false, error: "Could not read this page.", hasNext: true, hasPrevious: true };
  await render(<Paging label="Activity" active page={state} />);
  expect(screen.getByRole("alert")).toHaveTextContent("Could not read this page.");
  await fireEvent.press(screen.getByRole("button", { name: "Retry activity page" }));
  expect(state.retry).toHaveBeenCalledTimes(1);
  await fireEvent.press(screen.getByRole("button", { name: "First activity page" }));
  expect(state.first).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: "Previous activity" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Next activity" })).toBeDisabled();
});

test("initial reads show loading and inactive pages cannot navigate", async () => {
  const state = { ...page(), loaded: false, busy: true };
  const view = await render(<Paging label="Invitations" active page={state} />);
  expect(screen.getByLabelText("Loading invitations")).toBeTruthy();
  expect(screen.queryByRole("button")).toBeNull();
  await view.rerender(<Paging label="Invitations" active={false} page={{ ...state, loaded: true, busy: false, page: 2, hasNext: true, hasPrevious: true }} />);
  for (const control of screen.getAllByRole("button")) expect(control).toBeDisabled();
});
