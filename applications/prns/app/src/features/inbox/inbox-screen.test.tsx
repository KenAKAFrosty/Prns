import type {
  Contact,
  DevelopmentNodeSnapshot,
  LxmfMessage,
  LxmfPeerSummary,
  SendDirectTextOutcome,
} from "@prns-internal/expo";
import { fireEvent, render, waitFor } from "@testing-library/react-native";
import { destinationHash } from "personal-rns/contract";
import type { ReactNode } from "react";

import type { RuntimeCommandResult } from "@/native/development-runtime-context";
import { ComposeScreen, ConversationScreen, InboxScreen } from "./inbox-screen.ios";

const mockReplace = jest.fn();
const mockDestination = destinationHash(Uint8Array.from({ length: 16 }, (_, index) => index));
const mockPeer: LxmfPeerSummary = {
  destination: mockDestination,
  displayName: "Announced peer",
  requiredStampCost: null,
  lastObservedAgeMillis: 25n,
};
const mockMessage: LxmfMessage = {
  localRecordId: 1n,
  messageId: Uint8Array.from({ length: 32 }, () => 0x44),
  source: mockDestination,
  destination: destinationHash(Uint8Array.from({ length: 16 }, () => 0x55)),
  timestamp: 1_700_000_000_000n,
  title: { type: "utf8", value: "Questionable message" },
  content: { type: "invalidUtf8", bytes: Uint8Array.of(0xff, 0xfe) },
  direction: "inbound",
  verification: "invalidSignature",
  deliveryState: "received",
  failure: null,
};
const mockContact: Contact = {
  destination: mockDestination,
  identity: null,
  alias: "Saved alias",
  pinned: false,
};
const mockSnapshot: DevelopmentNodeSnapshot = {
  contractFingerprint: "test-contract",
  revision: 3n,
  runtime: "running",
  primaryIdentity: { type: "missing" },
  localHost: { type: "stopped", lastStartFailure: null },
  lxmf: { state: "ready", inboundOverflowCount: 0n },
  controllerIdentityFingerprint: null,
  pairing: { type: "searching" },
  pairedTargets: [],
  activeOperation: null,
  failure: null,
};

const mockListLxmfPeers = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: { type: "listed" as const, peers: [mockPeer] },
}));
const mockListLxmfMessages = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: { type: "listed" as const, messages: [mockMessage] },
}));
const mockMeasureLxmfText = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: { type: "measured" as const, wireBytes: 140, remainingBytes: 291 },
}));
const mockSendDirectText = jest.fn(
  async (): Promise<RuntimeCommandResult<SendDirectTextOutcome>> => ({
    type: "outcome",
    outcome: { type: "started", localRecordId: 2n },
  }),
);
const mockRefreshSnapshot = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: mockSnapshot,
}));
const mockAnnounceLxmf = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: { type: "announced" as const },
}));
const mockListContacts = jest.fn(async () => ({
  type: "listed" as const,
  contacts: [mockContact],
}));
const mockContactRuntime = { listContacts: mockListContacts };

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
  useRouter: () => ({ replace: mockReplace }),
}));

jest.mock("@/native/development-runtime-context", () => ({
  useDevelopmentRuntime: () => ({
    availability: { type: "available", platform: "ios" },
    phase: "ready",
    snapshot: mockSnapshot,
    lifecycleFailure: null,
    backgroundFailure: null,
    refreshSnapshot: mockRefreshSnapshot,
    listLxmfPeers: mockListLxmfPeers,
    listLxmfMessages: mockListLxmfMessages,
    announceLxmf: mockAnnounceLxmf,
    measureLxmfText: mockMeasureLxmfText,
    sendDirectText: mockSendDirectText,
  }),
}));

jest.mock("@/native/contact-runtime-context", () => ({
  useContactRuntime: () => ({
    availability: { type: "available", platform: "ios" },
    runtime: mockContactRuntime,
  }),
}));

beforeEach(() => {
  jest.clearAllMocks();
});

describe("in-memory LXMF screens", () => {
  test("uses saved alias before announced name and shows aggregate health", async () => {
    const screen = render(<InboxScreen />);

    await waitFor(() => {
      expect(screen.getByText("Saved alias")).toBeTruthy();
    });
    expect(screen.queryByText("Announced peer")).toBeNull();
    expect(screen.getByText("ready")).toBeTruthy();
    expect(mockListLxmfMessages).toHaveBeenCalledWith({
      peer: null,
      before: null,
      limit: 100,
    });
  });

  test("renders invalid UTF-8 and signature state and sends only through native measure", async () => {
    const screen = render(<ConversationScreen destination={mockDestination} />);

    await waitFor(() => {
      expect(screen.getByText(/Invalid UTF-8 \(2 bytes: ff fe\)/u)).toBeTruthy();
    });
    expect(screen.getByText("Unverified — invalid signature")).toBeTruthy();
    expect(screen.getByText("Received")).toBeTruthy();

    fireEvent.changeText(screen.getByLabelText("LXMF title"), "Hello");
    fireEvent.changeText(screen.getByLabelText("LXMF message"), "Proof please");
    await waitFor(() => {
      expect(screen.getByText(/140 encoded bytes/u)).toBeTruthy();
    });
    fireEvent.press(screen.getByText("Send direct message"));

    await waitFor(() => {
      expect(mockSendDirectText).toHaveBeenCalledWith({
        destination: mockDestination,
        title: "Hello",
        content: "Proof please",
      });
    });
    expect(await screen.findByText(/Started record 2/u)).toBeTruthy();
    expect(screen.queryByText(/^Delivered$/u)).toBeNull();
  });

  test("navigates from compose only after native starts a visible record", async () => {
    const destination = Array.from(mockDestination, (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
    const screen = render(<ComposeScreen initialDestination={destination} />);

    fireEvent.changeText(screen.getByLabelText("LXMF title"), "Hello");
    fireEvent.changeText(screen.getByLabelText("LXMF message"), "Proof please");
    await waitFor(() => {
      expect(screen.getByText(/140 encoded bytes/u)).toBeTruthy();
    });
    fireEvent.press(screen.getByText("Send direct message"));

    await waitFor(() => {
      expect(mockReplace).toHaveBeenCalledWith({
        pathname: "/inbox/conversation/[destination]",
        params: { destination },
      });
    });
  });

  test("keeps compose visible with an honest non-record send outcome", async () => {
    mockSendDirectText.mockResolvedValueOnce({
      type: "outcome",
      outcome: { type: "peerIdentityUnavailable" },
    });
    const destination = Array.from(mockDestination, (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
    const screen = render(<ComposeScreen initialDestination={destination} />);

    fireEvent.changeText(screen.getByLabelText("LXMF message"), "No observed peer");
    await waitFor(() => {
      expect(screen.getByText(/140 encoded bytes/u)).toBeTruthy();
    });
    fireEvent.press(screen.getByText("Send direct message"));

    expect(
      await screen.findByText("No compatible authenticated announce is available for this peer."),
    ).toBeTruthy();
    expect(screen.getByText("Compose")).toBeTruthy();
    expect(mockReplace).not.toHaveBeenCalled();
  });
});
