import * as Bindings from "@prns-internal/expo";
import { fireEvent, render } from "@testing-library/react-native";
import { RemoteChangeStatusCard, remoteChangeStatusMessage } from "./change-status";
import { ControllerAccessCard, type ControllerAccessCardProps } from "./controller-access";

const current = new Uint8Array(16).fill(1);
const other = new Uint8Array(16).fill(2);
const props: ControllerAccessCardProps = {
  controllerIdentityFingerprint: current,
  availableRequests: [
    Bindings.RemoteControlRequestKind.InventoryControllers,
    Bindings.RemoteControlRequestKind.RevokeController,
  ],
  busy: false,
  onLoad: jest.fn(),
  onLoadMore: jest.fn(),
  onChange: jest.fn(),
};

beforeEach(() => jest.clearAllMocks());

test("unsupported inventory stays hidden and unloaded inventory makes no empty claim", () => {
  const view = render(<ControllerAccessCard {...props} availableRequests={[]} />);
  expect(view.queryByText("Devices with access")).toBeNull();
  view.rerender(<ControllerAccessCard {...props} />);
  expect(view.queryByText("No devices were reported.")).toBeNull();
  fireEvent.press(view.getByRole("button", { name: "Load devices" }));
  expect(props.onLoad).toHaveBeenCalledTimes(1);
});

test("labels only this phone, does not fabricate other roles, and never offers self removal", () => {
  const view = render(
    <ControllerAccessCard {...props} page={{ identities: [current, other], next: undefined }} />,
  );
  expect(view.getByText("This phone")).toBeTruthy();
  expect(view.getByText("Device 02020202")).toBeTruthy();
  expect(view.queryByText("Operator")).toBeNull();
  expect(view.queryByText("Administrator")).toBeNull();
  expect(view.queryByRole("button", { name: "Remove access for 01010101" })).toBeNull();
  expect(view.getByRole("button", { name: "Remove access for 02020202" })).toBeTruthy();
});

test("removal requires explicit confirmation and does not optimistically delete the row", () => {
  const view = render(
    <ControllerAccessCard {...props} page={{ identities: [other], next: undefined }} />,
  );
  fireEvent.press(view.getByRole("button", { name: "Remove access for 02020202" }));
  expect(props.onChange).not.toHaveBeenCalled();
  fireEvent.press(view.getByRole("button", { name: "Cancel" }));
  expect(props.onChange).not.toHaveBeenCalled();
  fireEvent.press(view.getByRole("button", { name: "Remove access for 02020202" }));
  fireEvent.press(view.getByRole("button", { name: "Remove device access" }));
  expect(props.onChange).toHaveBeenCalledWith(
    Bindings.RemoteNodeChange.RevokeController.new({ controllerIdentityFingerprint: other }),
  );
  expect(view.getByText("Device 02020202")).toBeTruthy();
});

test("inventory-only access has no removal and empty results are explicit", () => {
  const view = render(
    <ControllerAccessCard
      {...props}
      availableRequests={[Bindings.RemoteControlRequestKind.InventoryControllers]}
      page={{ identities: [other], next: undefined }}
    />,
  );
  expect(view.queryByRole("button", { name: "Remove access for 02020202" })).toBeNull();
  view.rerender(<ControllerAccessCard {...props} page={{ identities: [], next: undefined }} />);
  expect(view.getByText("No devices were reported.")).toBeTruthy();
});

test("removal is unavailable until the current device identity is known", () => {
  const view = render(
    <ControllerAccessCard
      {...props}
      controllerIdentityFingerprint={new Uint8Array()}
      page={{ identities: [other], next: undefined }}
    />,
  );
  expect(view.queryByRole("button", { name: "Remove access for 02020202" })).toBeNull();
});

test("paging is explicit and busy disables loading and confirmed removal", () => {
  const page = { identities: [other], next: other };
  const view = render(<ControllerAccessCard {...props} page={page} />);
  fireEvent.press(view.getByRole("button", { name: "Load more devices" }));
  expect(props.onLoadMore).toHaveBeenCalledTimes(1);
  fireEvent.press(view.getByRole("button", { name: "Remove access for 02020202" }));
  view.rerender(<ControllerAccessCard {...props} page={page} busy />);
  expect(view.getByRole("button", { name: "Refresh devices" })).toBeDisabled();
  expect(view.getByRole("button", { name: "Load more devices" })).toBeDisabled();
  expect(view.getByRole("button", { name: "Remove device access" })).toBeDisabled();
  fireEvent.press(view.getByRole("button", { name: "Remove device access" }));
  expect(props.onChange).not.toHaveBeenCalled();
});

test("removal status distinguishes success, already removed, refusal and uncertainty", () => {
  const change = Bindings.RemoteNodeChange.RevokeController.new({
    controllerIdentityFingerprint: other,
  });
  expect(remoteChangeStatusMessage(Bindings.RemoteChangeStatus.Applied.new(), change)).toContain(
    "removed this device's access",
  );
  expect(remoteChangeStatusMessage(Bindings.RemoteChangeStatus.Unchanged.new(), change)).toBe(
    "This device's access was already removed.",
  );
  const refusal = Bindings.RemoteChangeStatus.Failed.new({
    stage: Bindings.RemoteManagementFailureStage.Permission,
    detail: "internal detail",
  });
  expect(remoteChangeStatusMessage(refusal, change)).toContain("cannot be removed remotely");
  expect(remoteChangeStatusMessage(refusal, change)).not.toContain("internal detail");
  expect(
    remoteChangeStatusMessage(
      Bindings.RemoteChangeStatus.OutcomeUnknown.new({
        reason: Bindings.RemoteControlAnnounceUnknownReason.ConnectionLost,
      }),
      change,
    ),
  ).toContain("Reconnect and refresh the device list before trying again.");
  expect(
    remoteChangeStatusMessage(
      Bindings.RemoteChangeStatus.Failed.new({
        stage: Bindings.RemoteManagementFailureStage.Busy,
        detail: "busy",
      }),
      change,
    ),
  ).toContain("Wait for it to finish");
});

test("last-change card labels device removal as an action", () => {
  const view = render(
    <RemoteChangeStatusCard
      operation={{
        operationId: 1n,
        generationId: 1n,
        targetIdentityFingerprint: new Uint8Array(16).fill(3),
        change: Bindings.RemoteNodeChange.RevokeController.new({
          controllerIdentityFingerprint: other,
        }),
        status: Bindings.RemoteChangeStatus.Unchanged.new(),
      }}
    />,
  );
  expect(view.getByText("Action")).toBeTruthy();
  expect(view.getByText("Remove device access")).toBeTruthy();
  expect(view.getByText("This device's access was already removed.")).toBeTruthy();
});
