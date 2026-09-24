import { fireEvent, render } from "@testing-library/react-native";

import { ConfirmAction } from "./confirm-action";

test("requires confirmation before applying a disruptive change", () => {
  const onConfirm = jest.fn();
  const view = render(
    <ConfirmAction
      label="Turn interface off"
      warning="This may disconnect the app. Use another connection to turn it back on."
      disabled={false}
      onConfirm={onConfirm}
    />,
  );
  fireEvent.press(view.getByRole("button", { name: "Turn interface off" }));
  expect(onConfirm).not.toHaveBeenCalled();
  expect(view.getByText(/This may disconnect/)).toBeTruthy();
  fireEvent.press(view.getByRole("button", { name: "Apply change" }));
  expect(onConfirm).toHaveBeenCalledTimes(1);
  expect(view.queryByRole("button", { name: "Apply change" })).toBeNull();
});

test("canceling a confirmation does not submit a change", () => {
  const onConfirm = jest.fn();
  const view = render(
    <ConfirmAction
      label="Sleep node"
      warning="Use its button to wake it."
      disabled={false}
      onConfirm={onConfirm}
    />,
  );
  fireEvent.press(view.getByRole("button", { name: "Sleep node" }));
  fireEvent.press(view.getByRole("button", { name: "Cancel" }));
  expect(onConfirm).not.toHaveBeenCalled();
  expect(view.queryByRole("button", { name: "Apply change" })).toBeNull();
});

test("a newly busy owner prevents an already open confirmation from submitting", () => {
  const onConfirm = jest.fn();
  const props = { label: "Pause radios", warning: "This may disconnect the app.", onConfirm };
  const view = render(<ConfirmAction {...props} disabled={false} />);
  fireEvent.press(view.getByRole("button", { name: "Pause radios" }));
  view.rerender(<ConfirmAction {...props} disabled />);
  fireEvent.press(view.getByRole("button", { name: "Apply change" }));
  expect(onConfirm).not.toHaveBeenCalled();
  fireEvent.press(view.getByRole("button", { name: "Cancel" }));
  expect(view.queryByRole("button", { name: "Apply change" })).toBeNull();
});
