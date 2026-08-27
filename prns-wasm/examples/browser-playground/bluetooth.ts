import { Tag, match } from "./sdk/index.js";
import type {
  BluetoothConnectOutcome,
  BluetoothInterface,
  BluetoothSession,
  InterfaceCloseOutcome,
} from "./sdk/index.js";
import {
  describeBluetoothConnectFailure,
  describeHostOperationFailure,
  describeInterfaceCloseFailure,
  describeSessionFailure,
  hostOperationFailed,
} from "./outcomes.js";
import type { BluetoothConnectFailure } from "./outcomes.js";
import { hex } from "./presentation.js";
import {
  bluetoothClosableSession,
  bluetoothConnectAvailable,
  bluetoothSession,
  observeBluetoothSession,
} from "./state.js";
import type { BluetoothState } from "./state.js";
import type { PlaygroundView } from "./view.js";

export class PlaygroundBluetoothController {
  readonly #interface: BluetoothInterface;
  readonly #view: PlaygroundView;
  readonly #onStateChanged: () => void;
  #state: BluetoothState = Tag("Waiting");
  #shutDown = false;

  constructor(
    bluetoothInterface: BluetoothInterface,
    view: PlaygroundView,
    onStateChanged: () => void,
  ) {
    this.#interface = bluetoothInterface;
    this.#view = view;
    this.#onStateChanged = onStateChanged;
  }

  get state(): BluetoothState {
    return this.#state;
  }

  start(): void {
    this.#transition(
      webBluetoothAvailable()
        ? Tag("Ready")
        : Tag("Unavailable", { api: "WebBluetooth" }),
    );
  }

  async connect(): Promise<void> {
    if (this.#shutDown || !bluetoothConnectAvailable(this.#state)) {
      return;
    }
    this.#transition(Tag("Connecting"));
    this.#view.record(
      "Bluetooth",
      "Device selection opened",
      "Choose an advertising Prns node in the browser prompt.",
    );
    let outcome: BluetoothConnectOutcome;
    try {
      outcome = await this.#interface.connect();
    } catch (error: unknown) {
      if (this.#shutDown) {
        return;
      }
      const failure = hostOperationFailed("Connect Bluetooth", error);
      this.#transition(Tag("ConnectFailed", failure));
      this.#view.record(
        "Failure",
        "Bluetooth did not connect",
        describeHostOperationFailure(failure),
      );
      return;
    }
    if (this.#shutDown) {
      if (outcome.tag === "Connected") {
        await outcome.data.close();
      }
      return;
    }
    match(outcome, {
      Connected: (session) => {
        this.#transition(Tag("Session", session));
        this.#view.record(
          "Bluetooth",
          "Session opened",
          `Interface ${hex(session.interfaceId)}`,
        );
      },
      HostApiUnavailable: (data) =>
        this.#connectFailed(Tag("HostApiUnavailable", data)),
      PermissionDenied: (data) =>
        this.#connectFailed(Tag("PermissionDenied", data)),
      Cancelled: (data) => this.#connectFailed(Tag("Cancelled", data)),
      UnsupportedDevice: (data) =>
        this.#connectFailed(Tag("UnsupportedDevice", data)),
      TimedOut: (data) => this.#connectFailed(Tag("TimedOut", data)),
      ConnectionFailed: (data) =>
        this.#connectFailed(Tag("ConnectionFailed", data)),
      AlreadyActive: (data) =>
        this.#connectFailed(Tag("AlreadyActive", data)),
      StableIdentityUnavailable: (data) =>
        this.#connectFailed(Tag("StableIdentityUnavailable", data)),
      RuntimeRejected: (data) =>
        this.#connectFailed(Tag("RuntimeRejected", data)),
    });
  }

  async close(): Promise<void> {
    if (this.#shutDown) {
      return;
    }
    const session = bluetoothClosableSession(this.#state);
    if (session === undefined) {
      return;
    }
    this.#transition(Tag("Closing", session));
    let outcome: InterfaceCloseOutcome;
    try {
      outcome = await session.close();
    } catch (error: unknown) {
      if (this.#shutDown) {
        return;
      }
      const failure = hostOperationFailed("Close Bluetooth", error);
      this.#transition(Tag("CloseFailed", { session, failure }));
      this.#view.record(
        "Failure",
        "Bluetooth close failed",
        describeHostOperationFailure(failure),
      );
      return;
    }
    if (this.#shutDown) {
      return;
    }
    match(outcome, {
      Closed: () => {
        this.#transition(Tag("Closed"));
        this.#view.record("Bluetooth", "Session closed", null);
      },
      CloseFailed: (data) => {
        const failure = Tag("CloseFailed", data);
        this.#transition(Tag("CloseFailed", { session, failure }));
        this.#view.record(
          "Failure",
          "Bluetooth close failed",
          describeInterfaceCloseFailure(failure),
        );
      },
    });
  }

  poll(): void {
    if (this.#state.tag !== "Session") {
      return;
    }
    const session = this.#state.data;
    match(observeBluetoothSession(session), {
      Live: () => this.#view.renderBluetooth(this.#state),
      Closed: () => {
        this.#transition(Tag("Closed"));
        this.#view.record(
          "Bluetooth",
          "Session closed by the transport",
          null,
        );
      },
      Failed: (failure) => {
        this.#transition(Tag("SessionFailed", failure));
        this.#view.record(
          "Failure",
          "Bluetooth session failed",
          describeSessionFailure(failure),
        );
      },
    });
  }

  async shutdown(): Promise<void> {
    this.#shutDown = true;
    const session = bluetoothSession(this.#state);
    this.#state = Tag("Closed");
    if (session !== undefined) {
      await session.close();
    }
  }

  #connectFailed(failure: BluetoothConnectFailure): void {
    this.#transition(Tag("ConnectFailed", failure));
    this.#view.record(
      "Failure",
      "Bluetooth did not connect",
      describeBluetoothConnectFailure(failure),
    );
  }

  #transition(state: BluetoothState): void {
    this.#state = state;
    this.#view.renderBluetooth(state);
    this.#onStateChanged();
  }
}

function webBluetoothAvailable(): boolean {
  const bluetooth = (
    navigator as Navigator & {
      readonly bluetooth?: { readonly requestDevice?: unknown };
    }
  ).bluetooth;
  return typeof bluetooth?.requestDevice === "function";
}
