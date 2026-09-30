import * as Bindings from "@prns-internal/expo";
import { fireEvent, render } from "@testing-library/react-native";
import { View } from "react-native";

import { formatRequestKind } from "./format";
import { PairingConfirmationCard } from "./pairing-confirmation-card";

const allPermissions = Object.values(Bindings.RemoteControlRequestKind).filter(
  (kind): kind is Bindings.RemoteControlRequestKind => typeof kind === "number",
);

function confirmation(
  attemptId = "attempt-1",
  permissions = allPermissions,
  authority = Bindings.RemoteControlControllerAuthority.Administrator,
) {
  return Bindings.RemoteControlPairingState.ConfirmationRequired.new({
    attemptId,
    authority,
    confirmationCode: "028831",
    permissions,
    targetIdentityFingerprint: new Uint8Array(16).fill(0x44),
  });
}

const idleProps = {
  pending: null,
  commandFailure: null,
  onApprove: jest.fn(),
  onReject: jest.fn(),
};

test("keeps the code, access summary and decisions visible with all controls collapsed", () => {
  const view = render(<PairingConfirmationCard {...idleProps} pairing={confirmation()} />);
  expect(view.getByLabelText("Confirmation code 0 2 8 8 3 1")).toBeTruthy();
  expect(view.getByText("Administrator")).toBeTruthy();
  expect(
    view.getByText("This device can also grant or remove access for other devices."),
  ).toBeTruthy();
  expect(view.getByText(/View node information\. Change node settings\./u)).toBeTruthy();
  expect(view.getByRole("button", { name: "Codes match — approve" })).toBeEnabled();
  expect(view.getByRole("button", { name: "Reject pairing" })).toBeEnabled();
  expect(
    view.getByRole("button", { name: "Show pairing details" }).props.accessibilityState,
  ).toMatchObject({
    expanded: false,
  });
  expect(view.queryByText("Node ID")).toBeNull();
  for (const permission of allPermissions) {
    expect(view.queryByText(formatRequestKind(permission))).toBeNull();
  }
});

test("shows every exact requested control on its own line only when expanded", () => {
  const view = render(<PairingConfirmationCard {...idleProps} pairing={confirmation()} />);
  fireEvent.press(view.getByRole("button", { name: "Show pairing details" }));
  expect(
    view.getByRole("button", { name: "Hide pairing details" }).props.accessibilityState,
  ).toMatchObject({
    expanded: true,
  });
  expect(view.getByText("44".repeat(16))).toBeTruthy();
  for (const permission of allPermissions) {
    expect(view.getByText(formatRequestKind(permission))).toBeTruthy();
  }
  fireEvent.press(view.getByRole("button", { name: "Hide pairing details" }));
  expect(view.queryByText("Requested controls")).toBeNull();
});

test("summarizes a limited operator grant without implying settings or administrator access", () => {
  const view = render(
    <PairingConfirmationCard
      {...idleProps}
      pairing={confirmation(
        "limited",
        [Bindings.RemoteControlRequestKind.Describe],
        Bindings.RemoteControlControllerAuthority.Operator,
      )}
    />,
  );
  expect(view.getByText("View node information.")).toBeTruthy();
  expect(view.queryByText(/Change node settings|grant or remove access|Full control/u)).toBeNull();
  fireEvent.press(view.getByRole("button", { name: "Show pairing details" }));
  expect(view.getByText("View node information")).toBeTruthy();
  expect(view.queryByText("Change Wi-Fi network")).toBeNull();
});

test("passes the current attempt to either decision and prevents duplicate pending decisions", () => {
  const onApprove = jest.fn();
  const onReject = jest.fn();
  const pairing = confirmation();
  const props = { ...idleProps, pairing, onApprove, onReject };
  const view = render(<PairingConfirmationCard {...props} />);
  fireEvent.press(view.getByRole("button", { name: "Codes match — approve" }));
  fireEvent.press(view.getByRole("button", { name: "Reject pairing" }));
  expect(onApprove).toHaveBeenCalledWith(pairing);
  expect(onReject).toHaveBeenCalledWith(pairing);
  view.rerender(
    <PairingConfirmationCard {...props} pending="approve" commandFailure="Try again." />,
  );
  expect(view.getByRole("button", { name: "Approving…" })).toBeDisabled();
  expect(view.getByRole("button", { name: "Reject pairing" })).toBeDisabled();
  expect(view.getByText("Try again.")).toBeTruthy();
});

test("collapses details when keyed to a new pairing attempt", () => {
  const view = render(
    <View>
      <PairingConfirmationCard {...idleProps} key="first" pairing={confirmation("first")} />
    </View>,
  );
  fireEvent.press(view.getByRole("button", { name: "Show pairing details" }));
  view.rerender(
    <View>
      <PairingConfirmationCard {...idleProps} key="second" pairing={confirmation("second")} />
    </View>,
  );
  expect(
    view.getByRole("button", { name: "Show pairing details" }).props.accessibilityState,
  ).toMatchObject({
    expanded: false,
  });
});
