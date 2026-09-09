import * as Bindings from "@prns-internal/expo";
import type { DevelopmentNodeSnapshot, DevelopmentRuntime } from "@prns-internal/expo";
import { fireEvent, render, waitFor } from "@testing-library/react-native";
import { destinationHash, identityHash } from "personal-rns/contract";
import type { ReactNode } from "react";
import {
  AddContactScreen,
  ContactDetailScreen,
  ContactsScreen,
} from "@/features/contacts/contacts-screen";
import { ContactRuntimeProvider } from "@/native/contact-runtime-context";
const mockReplace = jest.fn();
jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
  useRouter: () => ({ replace: mockReplace }),
}));
const destination = destinationHash(Uint8Array.from({ length: 16 }, (_, index) => index));
const identity = identityHash(Uint8Array.from({ length: 16 }, (_, index) => index + 16));
function snapshot(): DevelopmentNodeSnapshot {
  return {
    contractFingerprint: "test",
    revision: 0n,
    runtime: Bindings.DevelopmentNodeRuntime.Stopped,
    primaryIdentity: Bindings.PrimaryIdentityState.Missing.new(),
    localHost: Bindings.LocalHostState.Stopped.new({
      lastStartFailure: undefined,
    }),
    lxmf: { state: Bindings.LxmfHealthState.Stopped, inboundOverflowCount: 0n },
    controllerIdentityFingerprint: undefined,
    pairing: Bindings.RemoteControlPairingState.Searching.new(),
    pairingCandidates: [],
    pairedTargets: [],
    lastAnnouncement: undefined,
    generationId: 0n,
    activeOperation: undefined,
    failure: undefined,
  };
}
function fakeRuntime(overrides: Partial<DevelopmentRuntime> = {}): DevelopmentRuntime {
  return {
    inspectDevelopmentIdentity: async () => Bindings.PrimaryIdentityState.Missing.new(),
    previewIdentityImport: async () => Bindings.IdentityImportPreviewOutcome.InvalidLength.new(),
    createGeneratedIdentity: async () => Bindings.IdentityCreationOutcome.AlreadyExists.new(),
    createImportedIdentity: async () => Bindings.IdentityCreationOutcome.AlreadyExists.new(),
    startDevelopmentNode: async () =>
      Bindings.DevelopmentNodeStartOutcome.Started.new({
        snapshot: snapshot(),
      }),
    readDevelopmentNodeSnapshot: async () => snapshot(),
    initiateRemoteControlPairing: async () =>
      Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    approveRemoteControlPairing: async () => Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    rejectRemoteControlPairing: async () => Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    describeRemoteControlTarget: async () => Bindings.RemoteControlDescribeOutcome.Busy.new(),
    announceRemoteControlTarget: async () => Bindings.RemoteControlAnnounceOutcome.Busy.new(),
    saveObservedDestination: async () => Bindings.ContactMutationOutcome.NotObserved.new(),
    createManualContact: async () => Bindings.ContactMutationOutcome.NotFound.new(),
    setContactAlias: async () => Bindings.ContactMutationOutcome.NotFound.new(),
    setContactPinned: async () => Bindings.ContactMutationOutcome.NotFound.new(),
    deleteContact: async () => Bindings.ContactMutationOutcome.NotFound.new(),
    getContact: async () => Bindings.ContactLookupOutcome.NotFound.new(),
    listContacts: async () =>
      Bindings.ContactListOutcome.Listed.new({
        contacts: [],
      }),
    listLxmfPeers: async () =>
      Bindings.LxmfPeerListOutcome.Listed.new({
        peers: [],
      }),
    listLxmfMessages: async () =>
      Bindings.LxmfMessageListOutcome.Listed.new({
        messages: [],
      }),
    retryLxmfMessage: async () => Bindings.RetryLxmfMessageOutcome.NotFound.new(),
    cancelLxmfMessage: async () => Bindings.CancelLxmfMessageOutcome.NotFound.new(),
    announceLxmf: async () => Bindings.AnnounceLxmfOutcome.Announced,
    measureLxmfText: async () =>
      Bindings.MeasureLxmfTextOutcome.Measured.new({
        wireBytes: 113,
        remainingBytes: 318,
      }),
    sendDirectText: async () =>
      Bindings.SendDirectTextOutcome.Accepted.new({
        localRecordId: 1n,
      }),
    stopDevelopmentNode: async () => Bindings.DevelopmentNodeStopOutcome.AlreadyStopped.new(),
    resetDevelopmentData: async () => Bindings.DevelopmentNodeStopOutcome.AlreadyStopped.new(),
    ...overrides,
  };
}
function withRuntime(runtime: DevelopmentRuntime, screen: ReactNode) {
  return render(
    <ContactRuntimeProvider
      provider={{ availability: { type: "available", platform: "ios" }, runtime }}
    >
      {screen}
    </ContactRuntimeProvider>,
  );
}
describe("contact screens", () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });
  it("renders persisted contacts returned by the direct native facade", async () => {
    const listContacts = jest.fn(async () =>
      Bindings.ContactListOutcome.Listed.new({
        contacts: [{ destination, identity, alias: "Alice", pinned: true }],
      }),
    );
    const view = withRuntime(fakeRuntime({ listContacts }), <ContactsScreen />);
    await waitFor(() => expect(view.getByText("Alice")).toBeTruthy());
    expect(view.getByText("000102030405060708090a0b0c0d0e0f")).toBeTruthy();
    expect(view.getByText("Pinned")).toBeTruthy();
    expect(listContacts).toHaveBeenCalledTimes(1);
  });
  it("does not expose native contact-list failure details", async () => {
    const listContacts = jest.fn(async () =>
      Bindings.ContactListOutcome.DevelopmentUnavailable.new({
        detail: "E290 upstream RemoteControl signed availability database owner stopped",
      }),
    );
    const view = withRuntime(fakeRuntime({ listContacts }), <ContactsScreen />);
    expect(await view.findByText("Contacts could not be loaded. Try again.")).toBeTruthy();
    expect(
      view.queryAllByText(/E290|signed availability|upstream RemoteControl|database owner/iu),
    ).toHaveLength(0);
    expect(listContacts).toHaveBeenCalledTimes(1);
  });
  it("passes explicit optional manual fields and routes by destination hash", async () => {
    const createManualContact = jest.fn(async () =>
      Bindings.ContactMutationOutcome.Saved.new({
        contact: { destination, identity: undefined, alias: "Alice", pinned: false },
      }),
    );
    const view = withRuntime(fakeRuntime({ createManualContact }), <AddContactScreen />);
    fireEvent.changeText(view.getByLabelText("Destination"), "000102030405060708090a0b0c0d0e0f");
    fireEvent.changeText(view.getByLabelText("Name (optional)"), " Alice ");
    fireEvent.press(view.getByRole("button", { name: "Save contact" }));
    await waitFor(() =>
      expect(createManualContact).toHaveBeenCalledWith(destination, undefined, " Alice "),
    );
    expect(mockReplace).toHaveBeenCalledWith({
      pathname: "/contacts/[destination]",
      params: { destination: "000102030405060708090a0b0c0d0e0f" },
    });
  });
  it("loads a destination-addressed detail and reports pin rejection honestly", async () => {
    const getContact = jest.fn(async () =>
      Bindings.ContactLookupOutcome.Found.new({
        contact: { destination, identity: undefined, alias: "Manual", pinned: false },
      }),
    );
    const setContactPinned = jest.fn(async () =>
      Bindings.ContactMutationOutcome.MissingIdentity.new(),
    );
    const view = withRuntime(
      fakeRuntime({ getContact, setContactPinned }),
      <ContactDetailScreen destination={destination} />,
    );
    await waitFor(() => expect(view.getByText("Manual")).toBeTruthy());
    fireEvent.press(view.getByRole("button", { name: "Pin contact" }));
    await waitFor(() =>
      expect(
        view.getByText("Add or discover an identity before pinning this contact."),
      ).toBeTruthy(),
    );
    expect(setContactPinned).toHaveBeenCalledWith(destination, true);
  });
  it("does not simulate contacts on an unsupported platform", () => {
    const view = render(
      <ContactRuntimeProvider
        provider={{
          availability: { type: "unavailable", platform: "web", reason: "notImplemented" },
        }}
      >
        <ContactsScreen />
      </ContactRuntimeProvider>,
    );
    expect(view.getByText("Contacts unavailable")).toBeTruthy();
    expect(view.getByText("Contacts are not available on web yet.")).toBeTruthy();
  });
});

test.each([
  [
    Bindings.NativeStoragePreparationOutcome.DevelopmentResetRequired.new({
      reason: "private database layout detail",
    }),
    "App reset required",
  ],
  [
    Bindings.NativeStoragePreparationOutcome.Unavailable.new({
      detail: "private database lock detail",
    }),
    "Storage unavailable",
  ],
] as const)("preserves bootstrap recovery and retry guidance %#", async (outcome, title) => {
  let ready = false;
  const listContacts = jest.fn(async () => {
    if (!ready) throw new Bindings.NativeStoragePreparationError(outcome);
    return Bindings.ContactListOutcome.Listed.new({ contacts: [] });
  });
  const view = withRuntime(fakeRuntime({ listContacts }), <ContactsScreen />);
  expect(await view.findByText(title)).toBeTruthy();
  if (outcome.tag === "DevelopmentResetRequired")
    expect(view.getByText("Open recovery")).toBeTruthy();
  expect(view.queryByText(/private database/)).toBeNull();
  expect(view.queryByText("Loading saved contacts…")).toBeNull();
  ready = true;
  fireEvent.press(view.getByRole("button", { name: "Refresh contacts" }));
  expect(await view.findByText("No saved contacts")).toBeTruthy();
  expect(view.queryByText(title)).toBeNull();
});
