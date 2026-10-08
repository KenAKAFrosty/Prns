import type { AndroidRuntime, AndroidRuntimeStatus } from "@prns-internal/expo";
import { act, renderHook, waitFor } from "@testing-library/react-native";
import { AppState, type AppStateStatus } from "react-native";

import { useAndroidRuntime } from "./use-android-runtime";

const ready: AndroidRuntimeStatus = {
  revision: 2,
  bluetoothPermission: "granted",
  backgroundDiscovery: "notRequired",
  connectionNotification: "enabled",
  bluetoothRadio: "on",
  locationServices: "notRequired",
  service: "running",
  lastError: null,
};

function fixture(overrides: Partial<AndroidRuntime> = {}): AndroidRuntime {
  return {
    readStatus: jest.fn(async () => ready),
    requestBluetoothPermissions: jest.fn(async () => ready),
    requestBackgroundBluetoothPermission: jest.fn(async () => ready),
    requestConnectionNotificationPermission: jest.fn(async () => ready),
    addStatusListener: jest.fn(() => ({ remove: jest.fn() })),
    ...overrides,
  };
}

afterEach(() => jest.restoreAllMocks());

test("refreshes on resume without requesting permissions, and releases only subscriptions", async () => {
  let resume: ((state: AppStateStatus) => void) | undefined;
  const removeAppState = jest.fn();
  const removeStatus = jest.fn();
  jest.spyOn(AppState, "addEventListener").mockImplementation((_event, callback) => {
    resume = callback;
    return { remove: removeAppState };
  });
  const runtime = fixture({ addStatusListener: jest.fn(() => ({ remove: removeStatus })) });
  const view = renderHook(() => useAndroidRuntime(runtime));
  await waitFor(() => expect(view.result.current.status).toEqual(ready));
  await act(async () => resume?.("active"));
  expect(runtime.readStatus).toHaveBeenCalledTimes(2);
  expect(runtime.requestBluetoothPermissions).not.toHaveBeenCalled();
  expect(runtime.requestBackgroundBluetoothPermission).not.toHaveBeenCalled();
  expect(runtime.requestConnectionNotificationPermission).not.toHaveBeenCalled();
  view.unmount();
  expect(removeAppState).toHaveBeenCalledTimes(1);
  expect(removeStatus).toHaveBeenCalledTimes(1);
});

test("an older read cannot overwrite newer status or invent a read failure", async () => {
  let receive: ((status: AndroidRuntimeStatus) => void) | undefined;
  let rejectRead: ((error: Error) => void) | undefined;
  const runtime = fixture({
    readStatus: jest.fn(
      () =>
        new Promise((_resolve, reject) => {
          rejectRead = reject;
        }),
    ),
    addStatusListener: (listener) => {
      receive = listener;
      return { remove: jest.fn() };
    },
  });
  const view = renderHook(() => useAndroidRuntime(runtime));
  await act(async () => receive?.(ready));
  await act(async () => rejectRead?.(new Error("old read failed")));
  expect(view.result.current.status).toEqual(ready);
  expect(view.result.current.failure).toBeNull();
  await act(async () => receive?.({ ...ready, revision: 1, bluetoothRadio: "off" }));
  expect(view.result.current.status?.bluetoothRadio).toBe("on");
  view.unmount();
  await act(async () => receive?.({ ...ready, revision: 3, bluetoothRadio: "off" }));
});

test("permissions are explicit and background discovery is requested separately", async () => {
  const runtime = fixture();
  const view = renderHook(() => useAndroidRuntime(runtime));
  await waitFor(() => expect(view.result.current.status).toEqual(ready));
  await act(async () => view.result.current.requestBluetoothPermissions());
  expect(runtime.requestBluetoothPermissions).toHaveBeenCalledTimes(1);
  expect(runtime.requestBackgroundBluetoothPermission).not.toHaveBeenCalled();
  await act(async () => view.result.current.requestBackgroundBluetoothPermission());
  expect(runtime.requestBackgroundBluetoothPermission).toHaveBeenCalledTimes(1);
  expect(runtime.requestConnectionNotificationPermission).not.toHaveBeenCalled();
  await act(async () => view.result.current.requestConnectionNotificationPermission());
  expect(runtime.requestConnectionNotificationPermission).toHaveBeenCalledTimes(1);
});

test("a replaced runtime ignores an old pending read", async () => {
  let finish: ((status: AndroidRuntimeStatus) => void) | undefined;
  const oldRuntime = fixture({
    readStatus: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  });
  const current = fixture();
  const view = renderHook(
    ({ runtime }: { runtime: AndroidRuntime }) => useAndroidRuntime(runtime),
    {
      initialProps: { runtime: oldRuntime },
    },
  );
  view.rerender({ runtime: current });
  await waitFor(() => expect(view.result.current.status).toEqual(ready));
  await act(async () => finish?.({ ...ready, revision: 99, bluetoothRadio: "off" }));
  expect(view.result.current.status).toEqual(ready);
});
