import {
  Tag,
  appName,
  aspect,
  match,
  match_into,
} from "personal-rns/browser";
import type {
  AutoWifiControllerStatus,
  DeliveryEvidenceKind,
  DestinationHash,
  Prns,
  PrnsApplicationEvent,
  PrnsDiagnosticEvent,
  RuntimeRejected,
  Tag as Tagged,
} from "personal-rns/browser";
import type {
  DropAnnounceOutcome,
  DropAnnouncementState,
  DropContact,
  DropContactImportOutcome,
  DropContactPersistenceOutcome,
  DropContactReachability,
  DropForgetContactOutcome,
  DropHealth,
  DropIdentity,
  DropMessage,
  DropOutboundMessage,
  DropOutboundState,
  DropSendFailure,
  DropSendOutcome,
  DropSnapshot,
  DropStorageState,
  StoredDropContact,
} from "./model.js";
import type {
  DropContactStore,
  DropContactStoreLoadOutcome,
  DropContactStoreSaveOutcome,
} from "./contact_store.js";
import {
  MAX_DROP_TEXT_BYTES,
  decodeDropProfile,
  decodeDropTextMessage,
  encodeDropTextMessage,
  exportDropContactCode,
  hex,
  parseDropContactCode,
  prepareDropProfile,
} from "./protocol.js";
import type { DropProfileFailure } from "./protocol.js";

const AUTO_WIFI_POLL_INTERVAL_MS = 250;
const MESSAGE_ID_LENGTH = 16;
const MAX_MESSAGES = 200;
const MAX_SEEN_MESSAGE_IDS = 512;

type AutoWifiController = ReturnType<
  Prns["interfaces"]["autoWifi"]["start"]
>;

export type DropOpenOptions = {
  readonly displayName: string;
  readonly contactStore: DropContactStore;
  readonly now?: () => number;
};

export type DropOpenOutcome =
  | Tagged<"Opened", PrnsDrop>
  | DropProfileFailure
  | Tagged<"RegistrationRejected", RuntimeRejected["data"]>
  | Tagged<"ApplicationEventsUnavailable", { readonly lane: string }>;

type DropSubscriber = (snapshot: DropSnapshot) => void;

export class PrnsDrop {
  readonly #prns: Prns;
  readonly #contactStore: DropContactStore;
  readonly #now: () => number;
  readonly #autoWifi: AutoWifiController;
  readonly #applicationEvents: AsyncIterableIterator<PrnsApplicationEvent>;
  readonly #diagnosticEvents: AsyncIterableIterator<PrnsDiagnosticEvent> | undefined;
  readonly #contacts = new Map<string, DropContact>();
  readonly #subscribers = new Set<DropSubscriber>();
  readonly #seenMessageIds = new Set<string>();
  readonly #seenMessageOrder: string[] = [];
  #snapshot: DropSnapshot;
  #transportSignature: string;
  #announcedActiveSignature: string | undefined;
  #transportTimer: ReturnType<typeof globalThis.setInterval> | undefined;
  #announcementPromise: Promise<DropAnnounceOutcome> | undefined;
  #closed = false;

  private constructor(input: {
    readonly prns: Prns;
    readonly contactStore: DropContactStore;
    readonly now: () => number;
    readonly autoWifi: AutoWifiController;
    readonly applicationEvents: AsyncIterableIterator<PrnsApplicationEvent>;
    readonly diagnosticEvents: AsyncIterableIterator<PrnsDiagnosticEvent> | undefined;
    readonly identity: DropIdentity;
    readonly loadedContacts: DropContactStoreLoadOutcome;
  }) {
    this.#prns = input.prns;
    this.#contactStore = input.contactStore;
    this.#now = input.now;
    this.#autoWifi = input.autoWifi;
    this.#applicationEvents = input.applicationEvents;
    this.#diagnosticEvents = input.diagnosticEvents;
    if (input.loadedContacts.tag === "Loaded") {
      for (const stored of input.loadedContacts.data) {
        if (stored.destinationHex === input.identity.destinationHex) {
          continue;
        }
        this.#contacts.set(stored.destinationHex, {
          destination: stored.destination,
          destinationHex: stored.destinationHex,
          advertisedName: stored.displayName,
          persistence: Tag("Saved"),
          reachability: Tag("Unobserved"),
        });
      }
    }
    const transport = input.autoWifi.status;
    this.#transportSignature = autoWifiStatusSignature(transport);
    this.#snapshot = {
      lifecycle: Tag("Running"),
      identity: input.identity,
      transport,
      announcement: Tag("WaitingForTransport"),
      storage: storageState(input.loadedContacts),
      discovery: input.diagnosticEvents === undefined
        ? Tag("Unavailable", { lane: "Diagnostics" })
        : Tag("Listening"),
      health: Tag("Healthy"),
      contacts: this.#contactValues(),
      messages: [],
    };
  }

  static async open(prns: Prns, options: DropOpenOptions): Promise<DropOpenOutcome> {
    const profile = prepareDropProfile(options.displayName);
    if (profile.tag !== "Prepared") {
      return profile;
    }
    const registered = await prns.registerSingleDestination({
      appName: appName("prns"),
      aspects: [aspect("drop")],
      appData: profile.data.appData,
    });
    if (registered.tag !== "Registered") {
      return Tag("RegistrationRejected", registered.data);
    }
    const applicationEvents = prns.claimEvents();
    if (applicationEvents.tag !== "Claimed") {
      return Tag("ApplicationEventsUnavailable", {
        lane: applicationEvents.data.lane,
      });
    }
    const diagnosticClaim = prns.claimDiagnostics();
    const diagnosticEvents = diagnosticClaim.tag === "Claimed"
      ? diagnosticClaim.data
      : undefined;
    const loadedContacts = await options.contactStore.load();
    const destinationHex = hex(registered.data);
    const identity = {
      destination: registered.data,
      destinationHex,
      displayName: profile.data.displayName,
      contactCode: exportDropContactCode({
        destinationHex,
        displayName: profile.data.displayName,
      }),
    };
    const drop = new PrnsDrop({
      prns,
      contactStore: options.contactStore,
      now: options.now ?? Date.now,
      autoWifi: prns.interfaces.autoWifi.start(),
      applicationEvents: applicationEvents.data,
      diagnosticEvents,
      identity,
      loadedContacts,
    });
    drop.#start();
    return Tag("Opened", drop);
  }

  snapshot(): DropSnapshot {
    return this.#snapshot;
  }

  subscribe(subscriber: DropSubscriber): () => void {
    this.#subscribers.add(subscriber);
    subscriber(this.#snapshot);
    return () => {
      this.#subscribers.delete(subscriber);
    };
  }

  async importContact(code: string): Promise<DropContactImportOutcome> {
    const parsed = parseDropContactCode(code);
    if (parsed.tag !== "Parsed") {
      return parsed;
    }
    if (parsed.data.destinationHex === this.#snapshot.identity.destinationHex) {
      return Tag("SelfContact");
    }
    const current = this.#contacts.get(parsed.data.destinationHex);
    const candidate: DropContact = {
      destination: parsed.data.destination,
      destinationHex: parsed.data.destinationHex,
      advertisedName: parsed.data.displayName,
      persistence: Tag("Saved"),
      reachability: current?.reachability ?? Tag("Unobserved"),
    };
    const persisted = await this.#contactStore.save(
      this.#storedContacts(candidate),
    );
    const persistence = contactPersistenceOutcome(persisted);
    const contact: DropContact = persisted.tag === "Saved"
      ? candidate
      : { ...candidate, persistence: Tag("Transient") };
    this.#contacts.set(contact.destinationHex, contact);
    this.#replace({
      contacts: this.#contactValues(),
      storage: persisted.tag === "Saved"
        ? Tag("Available")
        : Tag("Unavailable", persisted.data),
    });
    return Tag("Imported", { contact, persistence });
  }

  async forgetContact(destinationHex: string): Promise<DropForgetContactOutcome> {
    const current = this.#contacts.get(destinationHex);
    if (current === undefined || current.persistence.tag !== "Saved") {
      return Tag("NotSaved");
    }
    const persisted = await this.#contactStore.save(
      this.#storedContacts(undefined, destinationHex),
    );
    if (persisted.tag !== "Saved") {
      this.#replace({ storage: Tag("Unavailable", persisted.data) });
      return Tag("StorageUnavailable", persisted.data);
    }
    if (current.reachability.tag === "Announced") {
      this.#contacts.set(destinationHex, {
        ...current,
        persistence: Tag("Transient"),
      });
    } else {
      this.#contacts.delete(destinationHex);
    }
    this.#replace({
      contacts: this.#contactValues(),
      storage: Tag("Available"),
    });
    return Tag("Forgotten");
  }

  announce(): Promise<DropAnnounceOutcome> {
    if (this.#closed) {
      return Promise.resolve(Tag("Closed"));
    }
    if (this.#snapshot.transport.tag !== "Active") {
      return Promise.resolve(Tag("TransportUnavailable"));
    }
    if (this.#announcementPromise !== undefined) {
      return this.#announcementPromise;
    }
    this.#announcementPromise = this.#performAnnounce().finally(() => {
      this.#announcementPromise = undefined;
    });
    return this.#announcementPromise;
  }

  async sendText(destinationHex: string, text: string): Promise<DropSendOutcome> {
    if (this.#closed) {
      return Tag("Rejected", Tag("Closed"));
    }
    const contact = this.#contacts.get(destinationHex);
    if (contact === undefined) {
      return Tag("Rejected", Tag("UnknownContact", { destinationHex }));
    }
    if (destinationHex === this.#snapshot.identity.destinationHex) {
      return Tag("Rejected", Tag("SelfDelivery"));
    }
    const messageId = this.#createMessageId();
    if (messageId.tag !== "Created") {
      return Tag("Rejected", messageId.data);
    }
    const sentAt = this.#now();
    const encoded = encodeDropTextMessage({
      id: messageId.data.bytes,
      sender: this.#snapshot.identity.destination,
      senderDisplayName: this.#snapshot.identity.displayName,
      sentAt,
      text,
    });
    const encodingFailure = dropEncodingFailure(encoded);
    if (encodingFailure !== undefined) {
      return Tag("Rejected", encodingFailure);
    }
    if (encoded.tag !== "Encoded") {
      return Tag("Rejected", Tag("UnexpectedFailure", {
        detail: `unhandled Drop encoding outcome ${encoded.tag}`,
      }));
    }
    const message: DropOutboundMessage = Tag("Outbound", {
      id: messageId.data.hex,
      peerDestinationHex: destinationHex,
      peerDisplayName: contact.advertisedName,
      text,
      sentAt,
      state: Tag("Sending"),
    });
    this.#replace({ messages: boundedMessages([...this.#snapshot.messages, message]) });
    try {
      const first = await this.#prns.sendSinglePacket(contact.destination, encoded.data);
      if (first.tag === "Succeeded") {
        return this.#deliver(message.data.id, first.data.data);
      }
      if (first.data.tag !== "NoRouteToDestination") {
        return this.#rejectSend(message.data.id, Tag("SendRejected", {
          failure: first.data,
        }));
      }
      this.#replaceOutboundState(message.data.id, Tag("DiscoveringPath"));
      const path = await this.#prns.requestPath(contact.destination);
      if (path.tag === "Failed") {
        return this.#rejectSend(message.data.id, Tag("PathDiscoveryRejected", {
          failure: path.data,
        }));
      }
      this.#replaceOutboundState(message.data.id, Tag("Sending"));
      const retry = await this.#prns.sendSinglePacket(contact.destination, encoded.data);
      if (retry.tag === "Failed") {
        return this.#rejectSend(message.data.id, Tag("RetryRejected", {
          failure: retry.data,
        }));
      }
      return this.#deliver(message.data.id, retry.data.data);
    } catch (error: unknown) {
      return this.#rejectSend(message.data.id, Tag("UnexpectedFailure", {
        detail: describeUnknown(error),
      }));
    }
  }

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    this.#replace({ lifecycle: Tag("Closing") });
    if (this.#transportTimer !== undefined) {
      globalThis.clearInterval(this.#transportTimer);
      this.#transportTimer = undefined;
    }
    await this.#autoWifi.close();
    this.#replace({
      lifecycle: Tag("Closed"),
      transport: Tag("Closed"),
      announcement: Tag("WaitingForTransport"),
    });
  }

  #start(): void {
    void this.#consumeApplicationEvents();
    if (this.#diagnosticEvents !== undefined) {
      void this.#consumeDiagnosticEvents(this.#diagnosticEvents);
    }
    this.#transportTimer = globalThis.setInterval(() => {
      this.#refreshTransport();
    }, AUTO_WIFI_POLL_INTERVAL_MS);
    this.#refreshTransport();
  }

  #refreshTransport(): void {
    if (this.#closed) {
      return;
    }
    const status = this.#autoWifi.status;
    const signature = autoWifiStatusSignature(status);
    if (signature !== this.#transportSignature) {
      this.#transportSignature = signature;
      this.#replace({ transport: status });
    }
    if (status.tag !== "Active") {
      this.#announcedActiveSignature = undefined;
      if (this.#snapshot.announcement.tag !== "WaitingForTransport") {
        this.#replace({ announcement: Tag("WaitingForTransport") });
      }
      return;
    }
    if (signature === this.#announcedActiveSignature) {
      return;
    }
    this.#announcedActiveSignature = signature;
    void this.announce();
  }

  async #performAnnounce(): Promise<DropAnnounceOutcome> {
    this.#replace({ announcement: Tag("Announcing") });
    try {
      const outcome = await this.#prns.announce(this.#snapshot.identity.destination);
      if (outcome.tag === "Failed") {
        this.#replace({ announcement: Tag("Failed", { failure: outcome.data }) });
        return Tag("Rejected", { failure: outcome.data });
      }
      this.#replace({ announcement: Tag("Announced", { announcedAt: this.#now() }) });
      return Tag("Announced");
    } catch (error: unknown) {
      const detail = describeUnknown(error);
      this.#replace({ announcement: Tag("Crashed", { detail }) });
      return Tag("UnexpectedFailure", { detail });
    }
  }

  async #consumeApplicationEvents(): Promise<void> {
    try {
      for await (const event of this.#applicationEvents) {
        if (this.#closed) {
          return;
        }
        match(event, {
          SingleDelivery: ({ destination, plaintext }) => {
            if (hex(destination) === this.#snapshot.identity.destinationHex) {
              this.#receive(plaintext);
            }
          },
          LinkDelivery: ({ plaintext }) => this.#receive(plaintext),
          Request: () => undefined,
          Response: () => undefined,
          ResponseSegment: () => undefined,
          ResourceAvailable: () => undefined,
          ResourceSegment: () => undefined,
          ResourceNeedsDecompression: () => undefined,
          ChannelMessage: () => undefined,
        });
      }
    } catch (error: unknown) {
      this.#failHealth(error);
    }
  }

  async #consumeDiagnosticEvents(
    events: AsyncIterableIterator<PrnsDiagnosticEvent>,
  ): Promise<void> {
    try {
      for await (const event of events) {
        if (this.#closed) {
          return;
        }
        match(event, {
          AnnounceHeard: ({ destination, hops, appData: advertised }) => {
            this.#hearAnnounce(destination, hops, advertised);
          },
          LinkEstablished: () => undefined,
          PeerIdentified: () => undefined,
          LinkClosed: () => undefined,
          LinkInterfaceMismatch: () => undefined,
          ResourceAssembled: () => undefined,
          ResourceFailed: () => undefined,
          ResourceSendProgress: () => undefined,
          SelfRatchetRotated: () => undefined,
          AnnounceHeldDropped: () => undefined,
          Delivered: () => undefined,
          RouteExpired: () => undefined,
          RouteEvicted: () => undefined,
          RouteInterfaceGone: () => undefined,
          RouteDropped: () => undefined,
          BackendDiagnostic: () => undefined,
          DiagnosticsDropped: () => undefined,
          PersistenceRestored: () => undefined,
          PersistenceFlushed: () => undefined,
          PersistenceFlushFailed: () => undefined,
        });
      }
    } catch (error: unknown) {
      this.#failHealth(error);
    }
  }

  #hearAnnounce(destination: DestinationHash, hops: number, advertised: Uint8Array): void {
    const destinationHex = hex(destination);
    if (destinationHex === this.#snapshot.identity.destinationHex) {
      return;
    }
    const profile = decodeDropProfile(advertised);
    if (profile.tag !== "Decoded") {
      return;
    }
    const current = this.#contacts.get(destinationHex);
    this.#contacts.set(destinationHex, {
      destination,
      destinationHex,
      advertisedName: profile.data.displayName,
      persistence: current?.persistence ?? Tag("Transient"),
      reachability: Tag("Announced", {
        hops,
        lastSeenAt: this.#now(),
      }),
    });
    this.#replace({ contacts: this.#contactValues() });
  }

  #receive(plaintext: Uint8Array): void {
    const decoded = decodeDropTextMessage(plaintext);
    if (decoded.tag !== "Decoded") {
      return;
    }
    if (
      decoded.data.senderHex === this.#snapshot.identity.destinationHex ||
      this.#seenMessageIds.has(`${decoded.data.senderHex}:${decoded.data.idHex}`)
    ) {
      return;
    }
    this.#rememberMessageId(`${decoded.data.senderHex}:${decoded.data.idHex}`);
    const contact = this.#contacts.get(decoded.data.senderHex);
    this.#contacts.set(decoded.data.senderHex, {
      destination: decoded.data.sender,
      destinationHex: decoded.data.senderHex,
      advertisedName: decoded.data.senderDisplayName,
      persistence: contact?.persistence ?? Tag("Transient"),
      reachability: contact?.reachability ?? Tag("Unobserved"),
    });
    const incoming: DropMessage = Tag("Inbound", {
      id: decoded.data.idHex,
      peerDestinationHex: decoded.data.senderHex,
      peerDisplayName: decoded.data.senderDisplayName,
      text: decoded.data.text,
      sentAt: decoded.data.sentAt,
      receivedAt: this.#now(),
    });
    this.#replace({
      contacts: this.#contactValues(),
      messages: boundedMessages([...this.#snapshot.messages, incoming]),
    });
  }

  #createMessageId():
    | Tagged<"Created", { readonly bytes: Uint8Array; readonly hex: string }>
    | Tagged<"Unavailable", DropSendFailure> {
    try {
      const bytes = new Uint8Array(MESSAGE_ID_LENGTH);
      globalThis.crypto.getRandomValues(bytes);
      return Tag("Created", { bytes, hex: hex(bytes) });
    } catch (error: unknown) {
      return Tag("Unavailable", Tag("EntropyUnavailable", {
        detail: describeUnknown(error),
      }));
    }
  }

  #deliver(
    messageId: string,
    delivered: {
      readonly rttMillis: number;
      readonly evidence: DeliveryEvidenceKind;
    },
  ): DropSendOutcome {
    this.#replaceOutboundState(messageId, Tag("Delivered", {
      deliveredAt: this.#now(),
      rttMillis: delivered.rttMillis,
      evidence: delivered.evidence,
    }));
    return Tag("Delivered", {
      messageId,
      rttMillis: delivered.rttMillis,
    });
  }

  #rejectSend(messageId: string, failure: DropSendFailure): DropSendOutcome {
    this.#replaceOutboundState(messageId, Tag("Failed", failure));
    return Tag("Rejected", failure);
  }

  #replaceOutboundState(messageId: string, state: DropOutboundState): void {
    this.#replace({
      messages: this.#snapshot.messages.map((message) =>
        message.tag === "Outbound" && message.data.id === messageId
          ? Tag("Outbound", { ...message.data, state })
          : message
      ),
    });
  }

  #rememberMessageId(messageId: string): void {
    this.#seenMessageIds.add(messageId);
    this.#seenMessageOrder.push(messageId);
    if (this.#seenMessageOrder.length <= MAX_SEEN_MESSAGE_IDS) {
      return;
    }
    const expired = this.#seenMessageOrder.shift();
    if (expired !== undefined) {
      this.#seenMessageIds.delete(expired);
    }
  }

  #storedContacts(
    candidate?: DropContact,
    excludedDestinationHex?: string,
  ): StoredDropContact[] {
    const stored = new Map<string, StoredDropContact>();
    for (const contact of this.#contacts.values()) {
      if (
        contact.persistence.tag === "Saved" &&
        contact.destinationHex !== excludedDestinationHex
      ) {
        stored.set(contact.destinationHex, {
          destination: contact.destination,
          destinationHex: contact.destinationHex,
          displayName: contact.advertisedName,
        });
      }
    }
    if (candidate !== undefined && candidate.destinationHex !== excludedDestinationHex) {
      stored.set(candidate.destinationHex, {
        destination: candidate.destination,
        destinationHex: candidate.destinationHex,
        displayName: candidate.advertisedName,
      });
    }
    return [...stored.values()];
  }

  #contactValues(): readonly DropContact[] {
    return [...this.#contacts.values()].sort(compareContacts);
  }

  #failHealth(error: unknown): void {
    const health: DropHealth = Tag("Failed", { detail: describeUnknown(error) });
    this.#replace({ health });
  }

  #replace(change: Partial<DropSnapshot>): void {
    this.#snapshot = { ...this.#snapshot, ...change };
    for (const subscriber of this.#subscribers) {
      subscriber(this.#snapshot);
    }
  }
}

function storageState(outcome: DropContactStoreLoadOutcome): DropStorageState {
  return outcome.tag === "Loaded"
    ? Tag("Available")
    : Tag("Unavailable", outcome.data);
}

function contactPersistenceOutcome(
  outcome: DropContactStoreSaveOutcome,
): DropContactPersistenceOutcome {
  return outcome.tag === "Saved"
    ? Tag("Saved")
    : Tag("SessionOnly", outcome.data);
}

function compareContacts(left: DropContact, right: DropContact): number {
  if (left.persistence.tag !== right.persistence.tag) {
    return left.persistence.tag === "Saved" ? -1 : 1;
  }
  if (left.reachability.tag !== right.reachability.tag) {
    return left.reachability.tag === "Announced" ? -1 : 1;
  }
  return left.advertisedName.localeCompare(right.advertisedName);
}

function boundedMessages(messages: readonly DropMessage[]): readonly DropMessage[] {
  return messages.length <= MAX_MESSAGES
    ? messages
    : messages.slice(messages.length - MAX_MESSAGES);
}

function dropEncodingFailure(
  outcome: ReturnType<typeof encodeDropTextMessage>,
): DropSendFailure | undefined {
  return match_into<DropSendFailure | undefined>().from(outcome, {
    Encoded: () => undefined,
    TextEmpty: () => Tag("EmptyText"),
    TextTooLong: ({ actualBytes, maximumBytes }) => Tag("TextTooLong", {
      actualBytes,
      maximumBytes,
    }),
    DisplayNameEmpty: () => Tag("UnexpectedFailure", {
      detail: "the active Drop identity has an empty display name",
    }),
    DisplayNameTooLong: ({ actualBytes, maximumBytes }) => Tag("UnexpectedFailure", {
      detail: `the active Drop identity is ${actualBytes} bytes; maximum ${maximumBytes}`,
    }),
    MessageIdInvalid: ({ actualBytes }) => Tag("UnexpectedFailure", {
      detail: `message ID is ${actualBytes} bytes; expected ${MESSAGE_ID_LENGTH}`,
    }),
    TimestampInvalid: ({ value }) => Tag("UnexpectedFailure", {
      detail: `message timestamp ${value} is invalid`,
    }),
  });
}

function autoWifiStatusSignature(status: AutoWifiControllerStatus): string {
  return match_into<string>().from(status, {
    Starting: () => "Starting",
    Discovering: ({ attempt }) => `Discovering:${attempt}`,
    Active: ({ gateways }) =>
      `Active:${gateways.map(({ id, url }) => `${id}:${url}`).join("|")}`,
    Unavailable: (failure) => `Unavailable:${JSON.stringify(failure)}`,
    Closed: () => "Closed",
  });
}

function describeUnknown(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
