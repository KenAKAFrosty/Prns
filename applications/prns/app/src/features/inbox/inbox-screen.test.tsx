import * as Bindings from "@prns-internal/expo";
import type {
  Contact,
  DevelopmentNodeSnapshot,
  LxmfMessage,
  LxmfMessageListOutcome,
  LxmfPeerSummary,
  MeasureLxmfTextOutcome,
  SendDirectTextOutcome,
} from "@prns-internal/expo";
import { fireEvent, render, waitFor } from "@testing-library/react-native";
import { destinationHash } from "personal-rns/contract";
import type { ReactNode } from "react";
import type { RuntimeCommandResult } from "@/native/development-runtime-context";
import { ComposeScreen, ConversationScreen, InboxScreen } from "./inbox-screen.native";
const mockReplace = jest.fn();
const mockDestination = destinationHash(Uint8Array.from({ length: 16 }, (_, index) => index));
const mockPeer: LxmfPeerSummary = {
  destination: mockDestination,
  displayName: "Announced peer",
  requiredStampCost: undefined,
  lastObservedAgeMillis: 25n,
};
const mockMessage: LxmfMessage = {
  localRecordId: 1n,
  messageId: Uint8Array.from({ length: 32 }, () => 0x44),
  source: mockDestination,
  destination: destinationHash(Uint8Array.from({ length: 16 }, () => 0x55)),
  timestamp: 1700000000000n,
  title: Bindings.LxmfText.Utf8.new({
    value: "Questionable message",
  }),
  content: Bindings.LxmfText.InvalidUtf8.new({
    bytes: Uint8Array.of(0xff, 0xfe),
  }),
  direction: Bindings.LxmfDirection.Inbound,
  verification: Bindings.LxmfVerification.InvalidSignature,
  deliveryState: Bindings.LxmfDeliveryState.Received.new(),
};
const mockFailedMessage: LxmfMessage = {
  ...mockMessage,
  localRecordId: 7n,
  messageId: Uint8Array.from({ length: 32 }, () => 0x77),
  source: destinationHash(Uint8Array.from({ length: 16 }, () => 0x55)),
  destination: mockDestination,
  title: Bindings.LxmfText.Utf8.new({
    value: "Retry me",
  }),
  content: Bindings.LxmfText.Utf8.new({
    value: "The signed wire must stay identical",
  }),
  direction: Bindings.LxmfDirection.Outbound,
  verification: Bindings.LxmfVerification.Verified,
  deliveryState: Bindings.LxmfDeliveryState.Failed.new({
    failedAttempts: 1n,
    lastFailure: Bindings.LxmfDeliveryFailure.DeliveryTimedOut,
  }),
};
const mockQueuedMessage: LxmfMessage = {
  ...mockFailedMessage,
  localRecordId: 8n,
  messageId: Uint8Array.from({ length: 32 }, () => 0x88),
  title: Bindings.LxmfText.Utf8.new({
    value: "Cancel me",
  }),
  deliveryState: Bindings.LxmfDeliveryState.Queued.new({
    failedAttempts: 0n,
  }),
};
const mockContact: Contact = {
  destination: mockDestination,
  identity: undefined,
  alias: "Saved alias",
  pinned: false,
};
const mockSnapshot: DevelopmentNodeSnapshot = {
  contractFingerprint: "test-contract",
  revision: 3n,
  runtime: Bindings.DevelopmentNodeRuntime.Running,
  primaryIdentity: Bindings.PrimaryIdentityState.Missing.new(),
  localHost: Bindings.LocalHostState.Stopped.new({
    lastStartFailure: undefined,
  }),
  lxmf: { state: Bindings.LxmfHealthState.Ready, inboundOverflowCount: 0n },
  controllerIdentityFingerprint: undefined,
  pairing: Bindings.RemoteControlPairingState.Searching.new(),
  pairingCandidates: [],
  pairedTargets: [],
  lastAnnouncement: undefined,
  generationId: 0n,
  activeOperation: undefined,
  failure: undefined,
};
const mockListLxmfPeers = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: Bindings.LxmfPeerListOutcome.Listed.new({
    peers: [mockPeer],
  }),
}));
const mockListLxmfMessages = jest.fn(
  async (): Promise<RuntimeCommandResult<LxmfMessageListOutcome>> => ({
    type: "outcome",
    outcome: Bindings.LxmfMessageListOutcome.Listed.new({
      messages: [mockMessage],
    }),
  }),
);
const mockMeasureLxmfText = jest.fn(
  async (): Promise<RuntimeCommandResult<MeasureLxmfTextOutcome>> => ({
    type: "outcome",
    outcome: Bindings.MeasureLxmfTextOutcome.Measured.new({
      wireBytes: 140,
      remainingBytes: 291,
    }),
  }),
);
const mockSendDirectText = jest.fn(
  async (): Promise<RuntimeCommandResult<SendDirectTextOutcome>> => ({
    type: "outcome",
    outcome: Bindings.SendDirectTextOutcome.Accepted.new({
      localRecordId: 2n,
    }),
  }),
);
const mockRetryLxmfMessage = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: Bindings.RetryLxmfMessageOutcome.Accepted.new({
    localRecordId: 7n,
  }),
}));
const mockCancelLxmfMessage = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: Bindings.CancelLxmfMessageOutcome.Cancelled.new({
    localRecordId: 8n,
  }),
}));
const mockRefreshSnapshot = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: mockSnapshot,
}));
const mockAnnounceLxmf = jest.fn(async () => ({
  type: "outcome" as const,
  outcome: Bindings.AnnounceLxmfOutcome.Announced,
}));
const mockListContacts = jest.fn(async () =>
  Bindings.ContactListOutcome.Listed.new({
    contacts: [mockContact],
  }),
);
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
    outcome: Bindings.LxmfPeerListOutcome.Listed.new({
      peers: [mockPeer],
    }),
  });
  mockListLxmfMessages.mockResolvedValue({
    type: "outcome",
    outcome: Bindings.LxmfMessageListOutcome.Listed.new({
      messages: [mockMessage],
    }),
  });
  mockRetryLxmfMessage.mockResolvedValue({
    type: "outcome",
    outcome: Bindings.RetryLxmfMessageOutcome.Accepted.new({
      localRecordId: 7n,
    }),
  });
  mockCancelLxmfMessage.mockResolvedValue({
    type: "outcome",
    outcome: Bindings.CancelLxmfMessageOutcome.Cancelled.new({
      localRecordId: 8n,
    }),
  });
  mockSendDirectText.mockResolvedValue({
    type: "outcome",
    outcome: Bindings.SendDirectTextOutcome.Accepted.new({
      localRecordId: 2n,
    }),
  });
  mockMeasureLxmfText.mockResolvedValue({
    type: "outcome",
    outcome: Bindings.MeasureLxmfTextOutcome.Measured.new({
      wireBytes: 140,
      remainingBytes: 291,
    }),
  });
});
describe("durable LXMF screens", () => {
  test("states messaging limits and address sharing in user language", async () => {
    const screen = render(<InboxScreen />);
    expect(await screen.findByText("Saved alias")).toBeTruthy();
    expect(screen.getByText("Keep prns open for reliable message delivery.")).toBeTruthy();
    expect(
      screen.getByText(
        "Before exchanging messages with a new contact, share this device's messaging address.",
      ),
    ).toBeTruthy();
  });
  test("describes degraded messaging health without implementation details", async () => {
    mockActiveSnapshot = {
      ...mockSnapshot,
      lxmf: { state: Bindings.LxmfHealthState.Degraded, inboundOverflowCount: 0n },
    };
    const screen = render(<InboxScreen />);
    expect(
      await screen.findByText("Messages may be delayed until the connection recovers."),
    ).toBeTruthy();
    expect(screen.queryByText(/callback overflow|mailbox access/iu)).toBeNull();
  });
  test("does not expose native mailbox-list failure details", async () => {
    mockListLxmfMessages.mockResolvedValue({
      type: "outcome",
      outcome: Bindings.LxmfMessageListOutcome.DevelopmentUnavailable.new({
        detail: "E290 upstream RemoteControl signed availability mailbox generation mismatch",
      }),
    });
    const screen = render(<InboxScreen />);
    expect(await screen.findByText("Messages are not available right now.")).toBeTruthy();
    expect(
      screen.queryAllByText(/E290|signed availability|upstream RemoteControl|mailbox generation/iu),
    ).toHaveLength(0);
  });
  test("uses saved alias before announced name and shows aggregate health", async () => {
    const screen = render(<InboxScreen />);
    await waitFor(() => {
      expect(screen.getByText("Saved alias")).toBeTruthy();
    });
    expect(screen.queryByText("Announced peer")).toBeNull();
    expect(screen.getByText("Messaging ready")).toBeTruthy();
    expect(mockListLxmfMessages).toHaveBeenCalledWith({
      peer: undefined,
      before: undefined,
      limit: 100,
    });
  });
  test("keeps connection warnings expanded when messaging is degraded", async () => {
    mockActiveSnapshot = {
      ...mockSnapshot,
      lxmf: { state: Bindings.LxmfHealthState.Degraded, inboundOverflowCount: 1n },
    };
    const screen = render(<InboxScreen />);
    expect(await screen.findByText("Limited")).toBeTruthy();
    expect(screen.getByText("Messages may be delayed until the connection recovers.")).toBeTruthy();
    expect(screen.queryByText("Messaging ready")).toBeNull();
  });
  test("renders invalid UTF-8 and signature state and sends only through native measure", async () => {
    const screen = render(<ConversationScreen destination={mockDestination} />);
    await waitFor(() => {
      expect(screen.getByText("Unreadable text")).toBeTruthy();
    });
    expect(screen.getByText("Unverified — invalid signature")).toBeTruthy();
    expect(screen.getAllByText("Received")).toHaveLength(2);
    fireEvent.changeText(screen.getByLabelText("Title (optional)"), "Hello");
    fireEvent.changeText(screen.getByLabelText("Message"), "Proof please");
    await waitFor(() => {
      expect(screen.getByText("140 bytes used · 291 bytes available")).toBeTruthy();
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
  test("shows the measured byte budget and blocks an oversized direct message", async () => {
    mockMeasureLxmfText.mockResolvedValue({
      type: "outcome",
      outcome: Bindings.MeasureLxmfTextOutcome.NeedsResource.new({
        wireBytes: 432,
      }),
    });
    const screen = render(<ConversationScreen destination={mockDestination} />);
    fireEvent.changeText(screen.getByLabelText("Message"), "Too large");
    expect(
      await screen.findByText("432 bytes used. This message is too large for direct delivery."),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Send message" }).props.accessibilityState).toEqual(
      expect.objectContaining({ disabled: true }),
    );
    expect(mockSendDirectText).not.toHaveBeenCalled();
  });
  test("navigates from compose only after native durably accepts a visible record", async () => {
    const destination = Array.from(mockDestination, (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
    const screen = render(<ComposeScreen initialDestination={destination} />);
    fireEvent.changeText(screen.getByLabelText("Title (optional)"), "Hello");
    fireEvent.changeText(screen.getByLabelText("Message"), "Proof please");
    await waitFor(() => {
      expect(screen.getByText("140 bytes used · 291 bytes available")).toBeTruthy();
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
      outcome: Bindings.SendDirectTextOutcome.PeerIdentityUnavailable.new(),
    });
    const destination = Array.from(mockDestination, (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
    const screen = render(<ComposeScreen initialDestination={destination} />);
    fireEvent.changeText(screen.getByLabelText("Message"), "No observed peer");
    await waitFor(() => {
      expect(screen.getByText("140 bytes used · 291 bytes available")).toBeTruthy();
    });
    fireEvent.press(screen.getByText("Send message"));
    expect(await screen.findByText("This address is not ready to receive messages.")).toBeTruthy();
    expect(screen.getByText("Compose")).toBeTruthy();
    expect(mockReplace).not.toHaveBeenCalled();
  });
  test("retries a failed record by id without recomposing or calling send", async () => {
    mockListLxmfMessages.mockResolvedValue({
      type: "outcome",
      outcome: Bindings.LxmfMessageListOutcome.Listed.new({
        messages: [mockFailedMessage],
      }),
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
      outcome: Bindings.LxmfMessageListOutcome.Listed.new({
        messages: [mockQueuedMessage],
      }),
    });
    const screen = render(<ConversationScreen destination={mockDestination} />);
    expect(await screen.findByText("Queued")).toBeTruthy();
    fireEvent.press(screen.getByText("Cancel queued message"));
    await waitFor(() => expect(mockCancelLxmfMessage).toHaveBeenCalledWith(8n));
    expect(await screen.findByText("Message cancelled.")).toBeTruthy();
  });
  test("lists durable rows and exposes exact retry while node startup has failed", async () => {
    mockPhase = "failed";
    mockActiveSnapshot = { ...mockSnapshot, runtime: Bindings.DevelopmentNodeRuntime.Failed };
    mockLifecycleFailure = "The local node could not restore persistence.";
    mockListLxmfMessages.mockResolvedValue({
      type: "outcome",
      outcome: Bindings.LxmfMessageListOutcome.Listed.new({
        messages: [mockFailedMessage],
      }),
    });
    const screen = render(<ConversationScreen destination={mockDestination} />);
    expect(await screen.findByText("Retry me")).toBeTruthy();
    expect(screen.getByText("Messaging offline")).toBeTruthy();
    expect(screen.getByText("Retry message")).toBeTruthy();
    expect(screen.queryByText("New message")).toBeNull();
    expect(mockListLxmfPeers).not.toHaveBeenCalled();
    expect(mockListLxmfMessages).toHaveBeenCalledWith({
      peer: mockDestination,
      before: undefined,
      limit: 100,
    });
  });
});

test("retains the native storage reset outcome through an offline mailbox read", async () => {
  mockListLxmfMessages.mockResolvedValue({
    type: "operationFailure",
    detail: "storage bootstrap failed",
    storagePreparation: Bindings.NativeStoragePreparationOutcome.DevelopmentResetRequired.new({
      reason: "private storage detail",
    }),
  });
  const screen = render(<InboxScreen />);
  expect(await screen.findByText("App reset required")).toBeTruthy();
  expect(screen.getByText("Open recovery")).toBeTruthy();
  expect(screen.queryByText(/private storage detail|storage bootstrap failed/)).toBeNull();
});
