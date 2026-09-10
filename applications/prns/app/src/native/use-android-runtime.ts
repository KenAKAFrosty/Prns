import type { AndroidRuntime, AndroidRuntimeStatus } from "@prns-internal/expo";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AppState } from "react-native";

export type AndroidRuntimeView = {
  readonly status: AndroidRuntimeStatus | null;
  readonly failure: string | null;
  readonly refresh: () => Promise<void>;
  readonly requestBluetoothPermissions: () => Promise<void>;
  readonly requestBackgroundBluetoothPermission: () => Promise<void>;
  readonly requestConnectionNotificationPermission: () => Promise<void>;
};

export function useAndroidRuntime(runtime: AndroidRuntime | undefined): AndroidRuntimeView {
  const [status, setStatus] = useState<AndroidRuntimeStatus | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const active = useRef<{
    readonly runtime: AndroidRuntime;
    readonly revision: () => number;
    readonly publish: (next: AndroidRuntimeStatus) => void;
    readonly fail: (error: unknown) => void;
  } | null>(null);

  const invoke = useCallback(
    async (operation: (owner: AndroidRuntime) => Promise<AndroidRuntimeStatus>) => {
      const owner = active.current;
      if (owner === null) return;
      const revision = owner.revision();
      try {
        const next = await operation(owner.runtime);
        if (active.current === owner) owner.publish(next);
      } catch (error) {
        if (active.current === owner && owner.revision() === revision) owner.fail(error);
      }
    },
    [],
  );
  const refresh = useCallback(() => invoke((owner) => owner.readStatus()), [invoke]);
  const requestBluetoothPermissions = useCallback(
    () => invoke((owner) => owner.requestBluetoothPermissions()),
    [invoke],
  );
  const requestBackgroundBluetoothPermission = useCallback(
    () => invoke((owner) => owner.requestBackgroundBluetoothPermission()),
    [invoke],
  );
  const requestConnectionNotificationPermission = useCallback(
    () => invoke((owner) => owner.requestConnectionNotificationPermission()),
    [invoke],
  );

  useEffect(() => {
    setStatus(null);
    setFailure(null);
    if (runtime === undefined) return;
    let revision = -1;
    const owner = {
      runtime,
      revision: () => revision,
      publish: (next: AndroidRuntimeStatus) => {
        if (active.current !== owner || next.revision < revision) return;
        revision = next.revision;
        setStatus(next);
        setFailure(null);
      },
      fail: (error: unknown) => {
        setFailure(error instanceof Error ? error.message : "Device status could not be checked.");
      },
    };
    active.current = owner;
    const subscription = runtime.addStatusListener(owner.publish);
    const appState = AppState.addEventListener("change", (next) => {
      if (next === "active") void refresh();
    });
    void refresh();
    return () => {
      active.current = null;
      subscription.remove();
      appState.remove();
    };
  }, [runtime, refresh]);

  return useMemo(
    () => ({
      status,
      failure,
      refresh,
      requestBluetoothPermissions,
      requestBackgroundBluetoothPermission,
      requestConnectionNotificationPermission,
    }),
    [
      status,
      failure,
      refresh,
      requestBluetoothPermissions,
      requestBackgroundBluetoothPermission,
      requestConnectionNotificationPermission,
    ],
  );
}
