import {
  NATIVE_CONTRACT_FIXTURES,
  type BluetoothState,
  type DevelopmentNodeStartOutcome,
  type DevelopmentNodeStopOutcome,
  type RemoteControlDescribeOutcome,
  type RemoteControlPairingCommandOutcome,
  type RemoteControlPairingState,
} from "./contract.generated";

function fixturesOf<Union>(fixtures: readonly Union[]): readonly Union[] {
  return fixtures;
}

const fixtureGroups = [
  ["BluetoothState", fixturesOf<BluetoothState>(NATIVE_CONTRACT_FIXTURES.bluetoothStates)],
  [
    "RemoteControlPairingState",
    fixturesOf<RemoteControlPairingState>(NATIVE_CONTRACT_FIXTURES.pairingStates),
  ],
  [
    "DevelopmentNodeStartOutcome",
    fixturesOf<DevelopmentNodeStartOutcome>(NATIVE_CONTRACT_FIXTURES.startOutcomes),
  ],
  [
    "DevelopmentNodeStopOutcome",
    fixturesOf<DevelopmentNodeStopOutcome>(NATIVE_CONTRACT_FIXTURES.stopOutcomes),
  ],
  [
    "RemoteControlPairingCommandOutcome",
    fixturesOf<RemoteControlPairingCommandOutcome>(NATIVE_CONTRACT_FIXTURES.pairingOutcomes),
  ],
  [
    "RemoteControlDescribeOutcome",
    fixturesOf<RemoteControlDescribeOutcome>(NATIVE_CONTRACT_FIXTURES.describeOutcomes),
  ],
] as const;

describe("Rust-generated contract fixtures", () => {
  test.each(fixtureGroups)(
    "%s fixtures survive the exact JSON bridge projection",
    (_name, fixtures) => {
      for (const expected of fixtures) {
        const parsed: unknown = JSON.parse(JSON.stringify(expected));
        expect(parsed).toEqual(expected);
      }
    },
  );

  test("covers every closed tagged variant expected by foundation.1", () => {
    expect(NATIVE_CONTRACT_FIXTURES.bluetoothStates.map(({ type }) => type)).toEqual([
      "notCompiled",
      "preparing",
      "unavailable",
      "ready",
      "degraded",
      "disabled",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.pairingStates.map(({ type }) => type)).toEqual([
      "bluetoothUnavailable",
      "searching",
      "candidateObserved",
      "invitationSubmitted",
      "confirmationRequired",
      "awaitingTargetApproval",
      "persisting",
      "paired",
      "rejected",
      "expired",
      "cancelled",
      "failed",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.startOutcomes.map(({ type }) => type)).toEqual([
      "started",
      "alreadyRunning",
      "failed",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.stopOutcomes.map(({ type }) => type)).toEqual([
      "stopped",
      "alreadyStopped",
      "failed",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.pairingOutcomes.map(({ type }) => type)).toEqual([
      "accepted",
      "busy",
      "failed",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.describeOutcomes.map(({ type }) => type)).toEqual([
      "described",
      "busy",
      "failed",
    ]);
  });
});
