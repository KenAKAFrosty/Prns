import * as Bindings from "@prns-internal/expo";
import { fireEvent, render } from "@testing-library/react-native";

import { remoteChangeStatusMessage, remoteManagementFailureMessage } from "./change-status";
import { RemoteInterfaceCard, type RemoteInterfaceCardProps } from "./interface-card";
import { LoRaEditor } from "./lora-editor";
import { NodeControls } from "./node-controls";
import { NodeOverviewCard } from "./node-overview";

const interfaceId = new Uint8Array(8).fill(1);
const entry: Bindings.RemoteInterfaceEntry = {
  interfaceId,
  kind: "bluetooth-auto",
  mode: Bindings.RemoteInterfaceMode.Full,
  connection: Bindings.RemoteConnectionState.Connected,
  enabled: true,
  txBytes: 9007199254740993n,
  rxBytes: 3n,
  links: 2,
  rateBytesPerSec: 10,
};
const profile: Bindings.RemoteLoRaProfile = {
  region: Bindings.RemoteLoRaRegion.Us915,
  frequencyHz: 915000000,
  bandwidthHz: 250000,
  spreadingFactor: 7,
  codingRate: 5,
  txPowerDbm: 17,
  preambleSymbols: 18,
};
const details: Bindings.RemoteInterfaceDetails = {
  interfaceId,
  configuration: Bindings.RemoteInterfaceConfiguration.Available.new({
    card: {
      name: "Nearby connection",
      group: "default",
      configuration: "",
      failure: "",
      destinations: 3,
      transportedLinks: 2,
      loraProfile: undefined,
    },
  }),
  discoveryGroups: Bindings.RemoteDiscoveryGroups.Available.new({ groups: ["reticulum"] }),
};
const basicProps: RemoteInterfaceCardProps = {
  entry,
  availableRequests: [],
  busy: false,
  onLoadDetails: jest.fn(),
  onLoadPeers: jest.fn(),
  onChange: jest.fn(),
};

beforeEach(() => jest.clearAllMocks());

test("overview distinguishes an empty battery from an unreported battery", () => {
  const view = render(
    <NodeOverviewCard
      overview={{
        firmware: "1.2.3",
        interfaces: undefined,
        power: { batteryPercent: 0, externalPower: Bindings.RemoteExternalPower.Charging },
      }}
    />,
  );
  expect(view.getByText("0%")).toBeTruthy();
  expect(view.getByText("Connected · Charging")).toBeTruthy();
  view.rerender(
    <NodeOverviewCard
      overview={{
        firmware: undefined,
        interfaces: undefined,
        power: { batteryPercent: undefined, externalPower: Bindings.RemoteExternalPower.Unknown },
      }}
    />,
  );
  expect(view.queryByText("0%")).toBeNull();
  expect(view.getAllByText("Not reported")).toHaveLength(2);
});

test("unsupported node actions are absent and setters without getters are not switches", () => {
  const onChange = jest.fn();
  const view = render(<NodeControls availableRequests={[]} busy={false} onChange={onChange} />);
  expect(view.queryAllByRole("button")).toHaveLength(0);
  view.rerender(
    <NodeControls
      availableRequests={[Bindings.RemoteControlRequestKind.SetDisplayVisibility]}
      busy={false}
      onChange={onChange}
    />,
  );
  expect(view.queryAllByRole("switch")).toHaveLength(0);
  expect(view.queryByRole("button", { name: "Turn positioning on" })).toBeNull();
  fireEvent.press(view.getByRole("button", { name: "Show display" }));
  expect(onChange).toHaveBeenCalledWith(
    Bindings.RemoteNodeChange.DisplayVisibility.new({ visible: true }),
  );
});

test("sleep and radio-mode changes require confirmation and disclose recovery", () => {
  const onChange = jest.fn();
  const view = render(
    <NodeControls
      availableRequests={[
        Bindings.RemoteControlRequestKind.SetSystemPower,
        Bindings.RemoteControlRequestKind.SetEspRadioMode,
      ]}
      busy={false}
      onChange={onChange}
    />,
  );
  fireEvent.press(view.getByRole("button", { name: "Sleep node" }));
  expect(onChange).not.toHaveBeenCalled();
  expect(view.getByText(/wake it using its controls/)).toBeTruthy();
  fireEvent.press(view.getByRole("button", { name: "Put node to sleep" }));
  expect(onChange).toHaveBeenCalledWith(
    Bindings.RemoteNodeChange.SystemPower.new({ awake: false }),
  );
  fireEvent.press(view.getByRole("button", { name: "Use Wi-Fi hotspot" }));
  expect(view.getByText(/may disconnect Bluetooth/)).toBeTruthy();
  expect(onChange).toHaveBeenCalledTimes(1);
  fireEvent.press(view.getByRole("button", { name: "Apply change" }));
  expect(onChange).toHaveBeenLastCalledWith(
    Bindings.RemoteNodeChange.RadioMode.new({ mode: Bindings.RemoteRadioMode.AccessPoint }),
  );
});

test("all node actions are disabled while another operation owns the connection", () => {
  const onChange = jest.fn();
  const view = render(
    <NodeControls
      availableRequests={[
        Bindings.RemoteControlRequestKind.SetGnssPower,
        Bindings.RemoteControlRequestKind.SetDisplayAutoOff,
        Bindings.RemoteControlRequestKind.WakeRadios,
      ]}
      busy
      onChange={onChange}
    />,
  );
  for (const button of view.getAllByRole("button")) fireEvent.press(button);
  expect(onChange).not.toHaveBeenCalled();
});

test("interface readouts preserve exact traffic counts and hide unavailable actions", () => {
  const view = render(<RemoteInterfaceCard {...basicProps} />);
  expect(view.queryByText("Interface ID")).toBeNull();
  expect(view.queryByText("9007199254740993 / 3 bytes")).toBeNull();
  fireEvent.press(view.getByRole("button", { name: "Show connection details" }));
  expect(view.getByText("9007199254740993 / 3 bytes")).toBeTruthy();
  expect(view.getByText("Bluetooth")).toBeTruthy();
  expect(view.queryAllByRole("button")).toHaveLength(1);
});

test("interface shutdown confirms before submitting the exact interface id", () => {
  const onChange = jest.fn();
  const view = render(
    <RemoteInterfaceCard
      {...basicProps}
      availableRequests={[Bindings.RemoteControlRequestKind.SetInterfacePower]}
      onChange={onChange}
    />,
  );
  fireEvent.press(view.getByRole("button", { name: "Turn interface off" }));
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.press(view.getByRole("button", { name: "Turn off" }));
  expect(onChange).toHaveBeenCalledWith(
    Bindings.RemoteNodeChange.InterfacePower.new({ interfaceId, enabled: false }),
  );
  expect(view.getByText("On")).toBeTruthy();
});

test("removed interfaces cannot submit settings changes", () => {
  const view = render(
    <RemoteInterfaceCard
      {...basicProps}
      details={{
        interfaceId,
        configuration: Bindings.RemoteInterfaceConfiguration.UnknownInterface.new(),
        discoveryGroups: Bindings.RemoteDiscoveryGroups.Unavailable.new(),
      }}
      availableRequests={[
        Bindings.RemoteControlRequestKind.SetInterfacePower,
        Bindings.RemoteControlRequestKind.SetInterfaceMode,
      ]}
    />,
  );
  expect(view.getByText(/interface is no longer available/)).toBeTruthy();
  expect(view.queryByRole("button", { name: "Turn interface off" })).toBeNull();
  expect(view.queryByRole("button", { name: "Edit interface settings" })).toBeNull();
});

test("valid LoRa configuration remains editable when discovery groups do not recognize it", () => {
  const onChange = jest.fn();
  const loraDetails: Bindings.RemoteInterfaceDetails = {
    interfaceId,
    configuration: Bindings.RemoteInterfaceConfiguration.Available.new({
      card: {
        name: "LoRa radio",
        group: "",
        configuration: "",
        failure: "",
        destinations: 1,
        transportedLinks: 0,
        loraProfile: profile,
      },
    }),
    discoveryGroups: Bindings.RemoteDiscoveryGroups.UnknownInterface.new(),
  };
  const view = render(
    <RemoteInterfaceCard
      {...basicProps}
      entry={{ ...entry, kind: "lora" }}
      details={loraDetails}
      availableRequests={[
        Bindings.RemoteControlRequestKind.SetInterfacePower,
        Bindings.RemoteControlRequestKind.SetInterfaceGroup,
        Bindings.RemoteControlRequestKind.SetInterfaceLoRaProfile,
        Bindings.RemoteControlRequestKind.ReplaceInterfaceDiscoveryGroups,
      ]}
      onChange={onChange}
    />,
  );
  expect(view.queryByText(/interface is no longer available/)).toBeNull();
  expect(view.getByRole("button", { name: "Turn interface off" })).toBeEnabled();
  fireEvent.press(view.getByRole("button", { name: "Edit interface settings" }));
  expect(view.queryByLabelText("Interface group")).toBeNull();
  expect(view.queryByLabelText("Discovery groups (one per line)")).toBeNull();
  fireEvent.changeText(view.getByLabelText("Frequency (Hz)"), "916000000");
  fireEvent.press(view.getByRole("button", { name: "Save LoRa settings" }));
  fireEvent.press(view.getByRole("button", { name: "Apply change" }));
  expect(onChange).toHaveBeenCalledWith(
    Bindings.RemoteNodeChange.InterfaceLoRa.new({
      interfaceId,
      profile: { ...profile, frequencyHz: 916000000 },
    }),
  );
});

test("an unsupported groups query does not disable an otherwise inventoried interface", () => {
  const view = render(
    <RemoteInterfaceCard
      {...basicProps}
      entry={{ ...entry, kind: "usb-auto-device" }}
      details={{
        interfaceId,
        configuration: Bindings.RemoteInterfaceConfiguration.Unavailable.new(),
        discoveryGroups: Bindings.RemoteDiscoveryGroups.UnknownInterface.new(),
      }}
      availableRequests={[Bindings.RemoteControlRequestKind.SetInterfacePower]}
    />,
  );
  expect(view.queryByText(/interface is no longer available/)).toBeNull();
  expect(view.getByRole("button", { name: "Turn interface off" })).toBeEnabled();
});

test("an unconfigured LoRa radio requires explicit region and numeric settings", () => {
  const onChange = jest.fn();
  const view = render(
    <RemoteInterfaceCard
      {...basicProps}
      entry={{ ...entry, kind: "lora" }}
      details={{
        interfaceId,
        configuration: Bindings.RemoteInterfaceConfiguration.Available.new({
          card: {
            name: "LoRa radio",
            group: "",
            configuration: "Not configured",
            failure: "",
            destinations: 0,
            transportedLinks: 0,
            loraProfile: undefined,
          },
        }),
        discoveryGroups: Bindings.RemoteDiscoveryGroups.UnknownInterface.new(),
      }}
      availableRequests={[Bindings.RemoteControlRequestKind.SetInterfaceLoRaProfile]}
      onChange={onChange}
    />,
  );
  fireEvent.press(view.getByRole("button", { name: "Edit interface settings" }));
  expect(view.getByText("Enter settings for this radio.")).toBeTruthy();
  expect(view.getByLabelText("Frequency (Hz)").props.value).toBe("");
  expect(view.queryAllByRole("radio", { checked: true })).toHaveLength(0);
  expect(view.getByRole("button", { name: "Save LoRa settings" })).toBeDisabled();
  for (const [label, value] of [
    ["Frequency (Hz)", "915000000"],
    ["Spreading factor", "7"],
    ["Bandwidth (Hz)", "250000"],
    ["Coding rate denominator (5 for 4/5)", "5"],
    ["Transmit power (dBm)", "17"],
    ["Preamble symbols", "18"],
  ] as const) {
    fireEvent.changeText(view.getByLabelText(label), value);
  }
  expect(view.getByRole("button", { name: "Save LoRa settings" })).toBeDisabled();
  fireEvent.press(view.getByRole("radio", { name: "US 915" }));
  expect(view.getByRole("button", { name: "Save LoRa settings" })).toBeEnabled();
  fireEvent.press(view.getByRole("button", { name: "Save LoRa settings" }));
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.press(view.getByRole("button", { name: "Apply change" }));
  expect(onChange).toHaveBeenCalledWith(
    Bindings.RemoteNodeChange.InterfaceLoRa.new({ interfaceId, profile }),
  );
});

test("node-wide LoRa permission does not offer initial radio setup for Bluetooth", () => {
  const view = render(
    <RemoteInterfaceCard
      {...basicProps}
      details={details}
      availableRequests={[Bindings.RemoteControlRequestKind.SetInterfaceLoRaProfile]}
    />,
  );
  expect(view.queryByRole("button", { name: "Edit interface settings" })).toBeNull();
});

test("discovery group form preserves names for native validation and requires confirmation", () => {
  const onChange = jest.fn();
  const view = render(
    <RemoteInterfaceCard
      {...basicProps}
      details={details}
      availableRequests={[Bindings.RemoteControlRequestKind.ReplaceInterfaceDiscoveryGroups]}
      onChange={onChange}
    />,
  );
  fireEvent.press(view.getByRole("button", { name: "Edit interface settings" }));
  fireEvent.changeText(view.getByLabelText("Discovery groups (one per line)"), "camp\ntrail");
  fireEvent.press(view.getByRole("button", { name: "Save discovery groups" }));
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.press(view.getByRole("button", { name: "Apply change" }));
  expect(onChange).toHaveBeenCalledWith(
    Bindings.RemoteNodeChange.DiscoveryGroups.new({ interfaceId, groups: ["camp", "trail"] }),
  );
  expect(view.queryByLabelText("Discovery groups (one per line)")).toBeNull();
});

test("Wi-Fi actions belong only to the Wi-Fi interface and carry its identity", () => {
  const onChange = jest.fn();
  const props = {
    ...basicProps,
    availableRequests: [Bindings.RemoteControlRequestKind.SetStationUplink],
    onChange,
  };
  const view = render(<RemoteInterfaceCard {...props} />);
  expect(view.queryByRole("button", { name: "Enable Wi-Fi connection" })).toBeNull();
  view.rerender(<RemoteInterfaceCard {...props} entry={{ ...entry, kind: "auto-wifi" }} />);
  fireEvent.press(view.getByRole("button", { name: "Enable Wi-Fi connection" }));
  fireEvent.press(view.getByRole("button", { name: "Apply change" }));
  expect(onChange).toHaveBeenCalledWith(
    Bindings.RemoteNodeChange.StationUplink.new({ interfaceId, enabled: true }),
  );
});

test("peer paging requests more explicitly without hiding the current entries", () => {
  const onLoadMorePeers = jest.fn();
  const peers: Bindings.RemotePeerPage = {
    interfaceId,
    next: new Uint8Array(8).fill(2),
    entries: [
      {
        peerId: new Uint8Array(8).fill(3),
        connection: Bindings.RemoteConnectionState.Degraded,
        txBytes: 2n,
        rxBytes: 4n,
        links: 0,
        destinations: 2,
        rateBytesPerSec: 7,
        radio: Bindings.RemotePeerRadio.Measured.new({
          family: Bindings.RemoteRadioFamily.Bluetooth,
          rssiDbm: -70,
          snrQuarterDb: -5,
          qualityTenthsPercent: 0,
        }),
        details: "",
      },
    ],
  };
  const view = render(
    <RemoteInterfaceCard {...basicProps} peers={peers} onLoadMorePeers={onLoadMorePeers} />,
  );
  expect(view.getByText("Limited connection")).toBeTruthy();
  expect(view.getByText("-70 dBm · -1.25 dB signal-to-noise · 0% quality")).toBeTruthy();
  fireEvent.press(view.getByRole("button", { name: "Load more peers" }));
  expect(onLoadMorePeers).toHaveBeenCalledTimes(1);
  expect(view.getByText("2 / 4 bytes")).toBeTruthy();
});

test("LoRa fields submit a typed profile and do not parse a protocol string", () => {
  const onSave = jest.fn();
  const view = render(<LoRaEditor profile={profile} busy={false} onSave={onSave} />);
  fireEvent.changeText(view.getByLabelText("Frequency (Hz)"), "916000000");
  fireEvent.press(view.getByRole("button", { name: "Save LoRa settings" }));
  expect(onSave).not.toHaveBeenCalled();
  expect(
    view.getByText(/saves a custom radio profile and may replace automatic radio selection/u),
  ).toBeTruthy();
  fireEvent.press(view.getByRole("button", { name: "Apply change" }));
  expect(onSave).toHaveBeenCalledWith({ ...profile, frequencyHz: 916000000 });
});

test("LoRa form cannot silently wrap or truncate numbers at the generated bridge", () => {
  const onSave = jest.fn();
  const view = render(<LoRaEditor profile={profile} busy={false} onSave={onSave} />);
  for (const invalid of ["", "300", "7.5", "7x"]) {
    fireEvent.changeText(view.getByLabelText("Spreading factor"), invalid);
    fireEvent.press(view.getByRole("button", { name: "Save LoRa settings" }));
    expect(view.queryByRole("button", { name: "Apply change" })).toBeNull();
  }
  expect(onSave).not.toHaveBeenCalled();
});

test("results distinguish scheduled, unchanged and uncertain writes without leaking diagnostics", () => {
  expect(remoteChangeStatusMessage(Bindings.RemoteChangeStatus.Scheduled.new())).toMatch(
    /will apply it shortly/,
  );
  expect(remoteChangeStatusMessage(Bindings.RemoteChangeStatus.Unchanged.new())).toMatch(
    /Nothing changed/,
  );
  expect(
    remoteChangeStatusMessage(
      Bindings.RemoteChangeStatus.OutcomeUnknown.new({
        reason: Bindings.RemoteControlAnnounceUnknownReason.Timeout,
      }),
    ),
  ).toMatch(/may have applied.*not repeated/);
  expect(
    remoteChangeStatusMessage(
      Bindings.RemoteChangeStatus.Failed.new({
        stage: Bindings.RemoteManagementFailureStage.Persistence,
        detail: "E290 RemoteControl internal persistence actor failed",
      }),
    ),
  ).not.toMatch(/E290|RemoteControl|actor/);
  expect(remoteManagementFailureMessage(Bindings.RemoteManagementFailureStage.Rollback)).toMatch(
    /could not restore/,
  );
});
