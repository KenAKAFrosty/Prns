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
  deliveryState: { type: "received" },
};
const mockFailedMessage: LxmfMessage = {
  ...mockMessage,
  localRecordId: 7n,
  messageId: Uint8Array.from({ length: 32 }, () => 0x77),
  source: destinationHash(Uint8Array.from({ length: 16 }, () => 0x55)),
  destination: mockDestination,
  title: { type: "utf8", value: "Retry me" },
  content: { type: "utf8", value: "The signed wire must stay identical" },
  direction: "outbound",
  verification: "verified",
  deliveryState: {
    type: "failed",
    failedAttempts: 1n,
    lastFailure: "deliveryTimedOut",
  },
};
const mockQueuedMessage: LxmfMessage = {
  ...mockFailedMessage,
  localRecordId: 8n,
  messageId: Uint8Array.from({ length: 32 }, () => 0x88),
  title: { type: "utf8", value: "Cancel me" },
  deliveryState: { type: "queued", failedAttempts: 0n },
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
    outcome: { type: "accepted", localRecordId: 2n },
  }),
);
const mockRetryLxmfMessage = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: { type: "accepted" as const, localRecordId: 7n },
}));
const mockCancelLxmfMessage = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: { type: "cancelled" as const, localRecordId: 8n },
}));
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
let mockPhase: "unavailable" | "starting" | "ready" | "failed" = "ready";
let mockActiveSnapshot: DevelopmentNodeSnapshot = mockSnapshot;
let mockLifecycleFailure: string | null = null;

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
  useRouter: () => ({ replace: mockReplace }),
}));

jest.mock("@/native/development-runtime-context", () => ({
  useDevelopmentRuntime: () => ({
    availability: { type: "available", platform: "ios" },
    phase: mockPhase,
    snapshot: mockActiveSnapshot,
    lifecycleFailure: mockLifecycleFailure,
    backgroundFailure: null,
    refreshSnapshot: mockRefreshSnapshot,
    listLxmfPeers: mockListLxmfPeers,
    listLxmfMessages: mockListLxmfMessages,
    retryLxmfMessage: mockRetryLxmfMessage,
    cancelLxmfMessage: mockCancelLxmfMessage,
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
  mockPhase = "ready";
  mockActiveSnapshot = mockSnapshot;
  mockLifecycleFailure = null;
  mockListLxmfPeers.mockResolvedValue({
    type: "outcome",
    outcome: { type: "listed", peers: [mockPeer] },
  });
  mockListLxmfMessages.mockResolvedValue({
    type: "outcome",
    outcome: { type: "listed", messages: [mockMessage] },
  });
  mockRetryLxmfMessage.mockResolvedValue({
    type: "outcome",
    outcome: { type: "accepted", localRecordId: 7n },
  });
  mockCancelLxmfMessage.mockResolvedValue({
    type: "outcome",
    outcome: { type: "cancelled", localRecordId: 8n },
  });
  mockSendDirectText.mockResolvedValue({
    type: "outcome",
    outcome: { type: "accepted", localRecordId: 2n },
  });
});

describe("durable LXMF screens", () => {
  test("states messaging limits and address sharing in user language", async () => {
    const screen = render(<InboxScreen />);

    expect(await screen.findByText("Saved alias")).toBeTruthy();
    expect(screen.getByText("New messages arrive only while prns is open.")).toBeTruthy();
    expect(
      screen.getByText(
        "Before exchanging messages with a new contact, share this device's messaging address.",
      ),
    ).toBeTruthy();
  });

  test("describes degraded messaging health without implementation details", async () => {
    mockActiveSnapshot = {
      ...mockSnapshot,
      lxmf: { state: "degraded", inboundOverflowCount: 0n },
    };
    const screen = render(<InboxScreen />);

    expect(
      await screen.findByText("Messages may be delayed until the connection recovers."),
    ).toBeTruthy();
    expect(screen.queryByText(/callback overflow|mailbox access/iu)).toBeNull();
  });

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
    expect(screen.getAllByText("Received")).toHaveLength(2);

    fireEvent.changeText(screen.getByLabelText("Message title"), "Hello");
    fireEvent.changeText(screen.getByLabelText("Message"), "Proof please");
    await waitFor(() => {
      expect(screen.getByText("Ready to send.")).toBeTruthy();
    });
    fireEvent.press(screen.getByText("Send message"));

    await waitFor(() => {
      expect(mockSendDirectText).toHaveBeenCalledWith({
        destination: mockDestination,
        title: "Hello",
        content: "Proof please",
      });
    });
    expect(await screen.findByText("Message queued.")).toBeTruthy();
    expect(screen.queryByText(/^Delivered$/u)).toBeNull();
  });

  test("navigates from compose only after native durably accepts a visible record", async () => {
    const destination = Array.from(mockDestination, (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
    const screen = render(<ComposeScreen initialDestination={destination} />);

    fireEvent.changeText(screen.getByLabelText("Message title"), "Hello");
    fireEvent.changeText(screen.getByLabelText("Message"), "Proof please");
    await waitFor(() => {
      expect(screen.getByText("Ready to send.")).toBeTruthy();
    });
    fireEvent.press(screen.getByText("Send message"));

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

    fireEvent.changeText(screen.getByLabelText("Message"), "No observed peer");
    await waitFor(() => {
      expect(screen.getByText("Ready to send.")).toBeTruthy();
    });
    fireEvent.press(screen.getByText("Send message"));

    expect(await screen.findByText("This address is not ready to receive messages.")).toBeTruthy();
    expect(screen.getByText("Compose")).toBeTruthy();
    expect(mockReplace).not.toHaveBeenCalled();
  });

  test("retries a failed record by id without recomposing or calling send", async () => {
    mockListLxmfMessages.mockResolvedValue({
      type: "outcome",
      outcome: { type: "listed", messages: [mockFailedMessage] },
    });
    const screen = render(<ConversationScreen destination={mockDestination} />);

    expect(await screen.findByText(/Failed after 1 failed attempt/u)).toBeTruthy();
    fireEvent.press(screen.getByText("Retry message"));

    await waitFor(() => expect(mockRetryLxmfMessage).toHaveBeenCalledWith(7n));
    expect(mockSendDirectText).not.toHaveBeenCalled();
    expect(await screen.findByText("Message queued to retry.")).toBeTruthy();
  });

  test("cancels a queued durable record by id", async () => {
    mockListLxmfMessages.mockResolvedValue({
      type: "outcome",
      outcome: { type: "listed", messages: [mockQueuedMessage] },
    });
    const screen = render(<ConversationScreen destination={mockDestination} />);

    expect(await screen.findByText("Queued")).toBeTruthy();
    fireEvent.press(screen.getByText("Cancel queued message"));

    await waitFor(() => expect(mockCancelLxmfMessage).toHaveBeenCalledWith(8n));
    expect(await screen.findByText("Message cancelled.")).toBeTruthy();
  });

  test("lists durable rows and exposes exact retry while node startup has failed", async () => {
    mockPhase = "failed";
    mockActiveSnapshot = { ...mockSnapshot, runtime: "failed" };
    mockLifecycleFailure = "The local node could not restore persistence.";
    mockListLxmfMessages.mockResolvedValue({
      type: "outcome",
      outcome: { type: "listed", messages: [mockFailedMessage] },
    });
    const screen = render(<ConversationScreen destination={mockDestination} />);

    expect(await screen.findByText("Retry me")).toBeTruthy();
    expect(screen.getByText("Messaging offline")).toBeTruthy();
    expect(screen.getByText("Retry message")).toBeTruthy();
    expect(screen.queryByText("New message")).toBeNull();
    expect(mockListLxmfPeers).not.toHaveBeenCalled();
    expect(mockListLxmfMessages).toHaveBeenCalledWith({
      peer: mockDestination,
      before: null,
      limit: 100,
    });
  });
});
