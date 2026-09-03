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
    runtime: "stopped",
    primaryIdentity: { type: "missing" },
    localHost: { type: "stopped", lastStartFailure: null },
    lxmf: { state: "stopped", inboundOverflowCount: 0n },
    controllerIdentityFingerprint: null,
    pairing: { type: "searching" },
    pairedTargets: [],
    activeOperation: null,
    failure: null,
  };
}

function fakeRuntime(overrides: Partial<DevelopmentRuntime> = {}): DevelopmentRuntime {
  return {
    inspectDevelopmentIdentity: async () => ({ type: "missing" }),
    previewIdentityImport: async () => ({ type: "invalidLength" }),
    createGeneratedIdentity: async () => ({ type: "alreadyExists" }),
    createImportedIdentity: async () => ({ type: "alreadyExists" }),
    startDevelopmentNode: async () => ({ type: "started", snapshot: snapshot() }),
    readDevelopmentNodeSnapshot: async () => snapshot(),
    initiateRemoteControlPairing: async () => ({ type: "busy" }),
    approveRemoteControlPairing: async () => ({ type: "busy" }),
    rejectRemoteControlPairing: async () => ({ type: "busy" }),
    describeRemoteControlTarget: async () => ({ type: "busy" }),
    saveObservedDestination: async () => ({ type: "notObserved" }),
    createManualContact: async () => ({ type: "notFound" }),
    setContactAlias: async () => ({ type: "notFound" }),
    setContactPinned: async () => ({ type: "notFound" }),
    deleteContact: async () => ({ type: "notFound" }),
    getContact: async () => ({ type: "notFound" }),
    listContacts: async () => ({ type: "listed", contacts: [] }),
    listLxmfPeers: async () => ({ type: "listed", peers: [] }),
    listLxmfMessages: async () => ({ type: "listed", messages: [] }),
    retryLxmfMessage: async () => ({ type: "notFound" }),
    cancelLxmfMessage: async () => ({ type: "notFound" }),
    announceLxmf: async () => ({ type: "announced" }),
    measureLxmfText: async () => ({
      type: "measured",
      wireBytes: 113,
      remainingBytes: 318,
    }),
    sendDirectText: async () => ({ type: "accepted", localRecordId: 1n }),
    stopDevelopmentNode: async () => ({ type: "alreadyStopped" }),
    resetDevelopmentData: async () => ({ type: "alreadyStopped" }),
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
    const listContacts = jest.fn(async () => ({
      type: "listed" as const,
      contacts: [{ destination, identity, alias: "Alice", pinned: true }],
    }));
    const view = withRuntime(fakeRuntime({ listContacts }), <ContactsScreen />);

    await waitFor(() => expect(view.getByText("Alice")).toBeTruthy());
    expect(view.getByText("000102030405060708090a0b0c0d0e0f")).toBeTruthy();
    expect(view.getByText("Pinned")).toBeTruthy();
    expect(listContacts).toHaveBeenCalledTimes(1);
  });

  it("passes explicit nullable manual fields and routes by destination hash", async () => {
    const createManualContact = jest.fn(async () => ({
      type: "saved" as const,
      contact: { destination, identity: null, alias: "Alice", pinned: false },
    }));
    const view = withRuntime(fakeRuntime({ createManualContact }), <AddContactScreen />);

    fireEvent.changeText(
      view.getByLabelText("Destination hash"),
      "000102030405060708090a0b0c0d0e0f",
    );
    fireEvent.changeText(view.getByLabelText("Contact alias"), " Alice ");
    fireEvent.press(view.getByRole("button", { name: "Save manual contact" }));

    await waitFor(() =>
      expect(createManualContact).toHaveBeenCalledWith(destination, null, " Alice "),
    );
    expect(mockReplace).toHaveBeenCalledWith({
      pathname: "/contacts/[destination]",
      params: { destination: "000102030405060708090a0b0c0d0e0f" },
    });
  });

  it("loads a destination-addressed detail and reports pin rejection honestly", async () => {
    const getContact = jest.fn(async () => ({
      type: "found" as const,
      contact: { destination, identity: null, alias: "Manual", pinned: false },
    }));
    const setContactPinned = jest.fn(async () => ({ type: "missingIdentity" as const }));
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
