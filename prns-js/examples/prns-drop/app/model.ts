import type {
  AutoWifiControllerStatus,
  CommandFailure,
  DeliveryEvidenceKind,
  DestinationHash,
  Tag,
} from "personal-rns/browser";

export type DropIdentity = {
  readonly destination: DestinationHash;
  readonly destinationHex: string;
  readonly displayName: string;
  readonly contactCode: string;
};

export type DropContactPersistence =
  | Tag<"Transient">
  | Tag<"Saved">;

export type DropContactReachability =
  | Tag<"Unobserved">
  | Tag<
      "Announced",
      {
        readonly hops: number;
        readonly lastSeenAt: number;
      }
    >;

export type DropContact = {
  readonly destination: DestinationHash;
  readonly destinationHex: string;
  readonly advertisedName: string;
  readonly persistence: DropContactPersistence;
  readonly reachability: DropContactReachability;
};

export type StoredDropContact = {
  readonly destination: DestinationHash;
  readonly destinationHex: string;
  readonly displayName: string;
};

export type DropSendFailure =
  | Tag<"EmptyText">
  | Tag<
      "TextTooLong",
      { readonly actualBytes: number; readonly maximumBytes: number }
    >
  | Tag<"UnknownContact", { readonly destinationHex: string }>
  | Tag<"SelfDelivery">
  | Tag<"EntropyUnavailable", { readonly detail: string }>
  | Tag<"SendRejected", { readonly failure: CommandFailure }>
  | Tag<"PathDiscoveryRejected", { readonly failure: CommandFailure }>
  | Tag<"RetryRejected", { readonly failure: CommandFailure }>
  | Tag<"UnexpectedFailure", { readonly detail: string }>
  | Tag<"Closed">;

export type DropOutboundState =
  | Tag<"Sending">
  | Tag<"DiscoveringPath">
  | Tag<
      "Delivered",
      {
        readonly deliveredAt: number;
        readonly rttMillis: number;
        readonly evidence: DeliveryEvidenceKind;
      }
    >
  | Tag<"Failed", DropSendFailure>;

export type DropInboundMessage = Tag<
  "Inbound",
  {
    readonly id: string;
    readonly peerDestinationHex: string;
    readonly peerDisplayName: string;
    readonly text: string;
    readonly sentAt: number;
    readonly receivedAt: number;
  }
>;

export type DropOutboundMessage = Tag<
  "Outbound",
  {
    readonly id: string;
    readonly peerDestinationHex: string;
    readonly peerDisplayName: string;
    readonly text: string;
    readonly sentAt: number;
    readonly state: DropOutboundState;
  }
>;

export type DropMessage = DropInboundMessage | DropOutboundMessage;

export type DropAnnouncementState =
  | Tag<"WaitingForTransport">
  | Tag<"Announcing">
  | Tag<"Announced", { readonly announcedAt: number }>
  | Tag<"Failed", { readonly failure: CommandFailure }>
  | Tag<"Crashed", { readonly detail: string }>;

export type DropStorageState =
  | Tag<"Available">
  | Tag<"Unavailable", { readonly detail: string }>;

export type DropDiscoveryState =
  | Tag<"Listening">
  | Tag<"Unavailable", { readonly lane: string }>;

export type DropHealth =
  | Tag<"Healthy">
  | Tag<"Failed", { readonly detail: string }>;

export type DropLifecycle =
  | Tag<"Running">
  | Tag<"Closing">
  | Tag<"Closed">;

export type DropSnapshot = {
  readonly lifecycle: DropLifecycle;
  readonly identity: DropIdentity;
  readonly transport: AutoWifiControllerStatus;
  readonly announcement: DropAnnouncementState;
  readonly storage: DropStorageState;
  readonly discovery: DropDiscoveryState;
  readonly health: DropHealth;
  readonly contacts: readonly DropContact[];
  readonly messages: readonly DropMessage[];
};

export type DropSendOutcome =
  | Tag<
      "Delivered",
      { readonly messageId: string; readonly rttMillis: number }
    >
  | Tag<"Rejected", DropSendFailure>;

export type DropContactPersistenceOutcome =
  | Tag<"Saved">
  | Tag<"SessionOnly", { readonly detail: string }>;

export type DropContactImportOutcome =
  | Tag<
      "Imported",
      {
        readonly contact: DropContact;
        readonly persistence: DropContactPersistenceOutcome;
      }
    >
  | Tag<"InvalidContactCode", { readonly detail: string }>
  | Tag<"SelfContact">;

export type DropForgetContactOutcome =
  | Tag<"Forgotten">
  | Tag<"NotSaved">
  | Tag<"StorageUnavailable", { readonly detail: string }>;

export type DropAnnounceOutcome =
  | Tag<"Announced">
  | Tag<"Rejected", { readonly failure: CommandFailure }>
  | Tag<"TransportUnavailable">
  | Tag<"UnexpectedFailure", { readonly detail: string }>
  | Tag<"Closed">;
