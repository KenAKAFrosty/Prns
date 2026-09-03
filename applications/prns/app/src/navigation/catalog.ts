export type ScaffoldAvailability = "implementedScaffold" | "notYetImplemented";
export type NavigationSection =
  | "installation"
  | "inbox"
  | "contacts"
  | "nodes"
  | "explore"
  | "more";

type ScalarParam = {
  readonly kind: "scalar";
  readonly name: string;
  readonly required: boolean;
  readonly format?: "destinationHash";
};

type EnumParam = {
  readonly kind: "enum";
  readonly name: string;
  readonly required: boolean;
  readonly values: readonly string[];
};

type UnionParam = {
  readonly kind: "union";
  readonly name: string;
  readonly required: boolean;
  readonly literals?: readonly string[];
  readonly prefixes: readonly string[];
};

export type RouteParamRule = ScalarParam | EnumParam | UnionParam;

export type ScreenCatalogEntry = {
  readonly id: string;
  readonly path: string;
  readonly routeFiles: readonly string[];
  readonly label: string;
  readonly section: NavigationSection;
  readonly phonePlacement: string;
  readonly widePlacement: string;
  readonly deepLink: "external-navigation" | "app-issued" | "none";
  readonly availability: ScaffoldAvailability;
  readonly summary: string;
  readonly limitation: string;
  readonly backPath: string;
  readonly params: readonly RouteParamRule[];
  readonly phoneRootOrder?: number;
  readonly wideRootOrder?: number;
};

const noParams: readonly RouteParamRule[] = [];

export const screenCatalog = [
  {
    id: "installation.onboarding",
    path: "/onboarding/:step?",
    routeFiles: ["app/onboarding/index.tsx", "app/onboarding/[step].tsx"],
    label: "Onboarding",
    section: "installation",
    phonePlacement: "Full-screen gate",
    widePlacement: "Full-window gate",
    deepLink: "none",
    availability: "implementedScaffold",
    summary: "Set up the identity this device will use on Reticulum.",
    limitation: "Identity export, backup, and recovery are not available in this preview.",
    backPath: "/onboarding/welcome",
    params: [
      {
        kind: "enum",
        name: "step",
        required: false,
        values: [
          "welcome",
          "create",
          "import",
          "retention",
          "provision",
          "interfaces",
          "pairing",
          "complete",
        ],
      },
    ],
  },
  {
    id: "installation.recovery",
    path: "/recovery",
    routeFiles: ["app/recovery.tsx"],
    label: "Recovery",
    section: "installation",
    phonePlacement: "Full-screen gate",
    widePlacement: "Full-window gate",
    deepLink: "none",
    availability: "implementedScaffold",
    summary: "Check your identity or reset this app's data.",
    limitation: "Identity repair, export, and backup are not available in this preview.",
    backPath: "/onboarding/welcome",
    params: [{ kind: "scalar", name: "installationId", required: false }],
  },
  {
    id: "inbox.index",
    path: "/inbox",
    routeFiles: ["app/(shell)/inbox/index.tsx"],
    label: "Inbox",
    section: "inbox",
    phonePlacement: "Inbox tab root",
    widePlacement: "Inbox rail root",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "Read and manage messages saved on this device.",
    limitation: "Messaging is available only on iOS and only while prns is open.",
    backPath: "/inbox",
    params: noParams,
    phoneRootOrder: 0,
    wideRootOrder: 0,
  },
  {
    id: "inbox.conversation",
    path: "/inbox/conversation/:destination",
    routeFiles: ["app/(shell)/inbox/conversation/[destination].tsx"],
    label: "Conversation",
    section: "inbox",
    phonePlacement: "Push from Inbox",
    widePlacement: "Inbox detail pane",
    deepLink: "app-issued",
    availability: "implementedScaffold",
    summary: "Read messages with one contact and retry or cancel queued messages.",
    limitation: "Failed messages must be retried manually.",
    backPath: "/inbox",
    params: [{ kind: "scalar", name: "destination", required: true, format: "destinationHash" }],
  },
  {
    id: "inbox.message",
    path: "/inbox/message/:messageId",
    routeFiles: ["app/(shell)/inbox/message/[messageId].tsx"],
    label: "Message",
    section: "inbox",
    phonePlacement: "Push or sheet from conversation",
    widePlacement: "Inspector detail pane",
    deepLink: "app-issued",
    availability: "notYetImplemented",
    summary: "View message details and delivery status.",
    limitation: "Message details are not available yet.",
    backPath: "/inbox",
    params: [{ kind: "scalar", name: "messageId", required: true }],
  },
  {
    id: "inbox.compose",
    path: "/inbox/compose",
    routeFiles: ["app/(shell)/inbox/compose.tsx"],
    label: "Compose",
    section: "inbox",
    phonePlacement: "Full-screen composer",
    widePlacement: "Composer detail pane",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "Write a direct message to a Reticulum destination.",
    limitation: "Large messages are not supported yet.",
    backPath: "/inbox",
    params: [{ kind: "scalar", name: "destination", required: false, format: "destinationHash" }],
  },
  {
    id: "contacts.index",
    path: "/contacts",
    routeFiles: ["app/(shell)/contacts/index.tsx"],
    label: "Contacts",
    section: "contacts",
    phonePlacement: "Contacts tab root",
    widePlacement: "Contacts rail root",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "Browse contacts saved on this device.",
    limitation: "Contacts are available only on iOS in this preview.",
    backPath: "/contacts",
    params: noParams,
    phoneRootOrder: 1,
    wideRootOrder: 1,
  },
  {
    id: "contacts.entry",
    path: "/contacts/:destination",
    routeFiles: ["app/(shell)/contacts/[destination].tsx"],
    label: "Contact",
    section: "contacts",
    phonePlacement: "Push from Contacts",
    widePlacement: "Contacts detail pane",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "View or edit a saved contact.",
    limitation: "Saved contacts do not update automatically from the network.",
    backPath: "/contacts",
    params: [{ kind: "scalar", name: "destination", required: true, format: "destinationHash" }],
  },
  {
    id: "contacts.add",
    path: "/contacts/add",
    routeFiles: ["app/(shell)/contacts/add.tsx"],
    label: "Add contact",
    section: "contacts",
    phonePlacement: "Modal or full screen",
    widePlacement: "Dialog or detail pane",
    deepLink: "none",
    availability: "implementedScaffold",
    summary: "Save a destination with an optional identity and name.",
    limitation: "Verified destinations can also be saved from Nodes > This device.",
    backPath: "/contacts",
    params: noParams,
  },
  {
    id: "contacts.merge",
    path: "/contacts/merge",
    routeFiles: ["app/(shell)/contacts/merge.tsx"],
    label: "Merge contacts",
    section: "contacts",
    phonePlacement: "Full-screen review",
    widePlacement: "Dialog",
    deepLink: "none",
    availability: "notYetImplemented",
    summary: "Combine two saved contacts.",
    limitation: "Contact merging is not available yet.",
    backPath: "/contacts",
    params: [
      { kind: "scalar", name: "sourceId", required: true },
      { kind: "scalar", name: "targetId", required: true },
    ],
  },
  {
    id: "nodes.index",
    path: "/nodes",
    routeFiles: ["app/(shell)/nodes/index.tsx"],
    label: "Nodes",
    section: "nodes",
    phonePlacement: "Nodes tab root",
    widePlacement: "Nodes rail root",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "Manage this device and its paired nodes.",
    limitation: "Node management is available only on iOS in this preview.",
    backPath: "/nodes",
    params: noParams,
    phoneRootOrder: 2,
    wideRootOrder: 2,
  },
  {
    id: "nodes.local",
    path: "/nodes/local",
    routeFiles: ["app/(shell)/nodes/local/index.tsx"],
    label: "This device",
    section: "nodes",
    phonePlacement: "Push from Nodes",
    widePlacement: "Nodes detail pane",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "View detailed network status for this device.",
    limitation: "Detailed node status is available only on iOS in this preview.",
    backPath: "/nodes",
    params: noParams,
  },
  {
    id: "nodes.managed",
    path: "/nodes/managed/:nodeId",
    routeFiles: ["app/(shell)/nodes/managed/[nodeId].tsx"],
    label: "Manage node",
    section: "nodes",
    phonePlacement: "Push from Nodes",
    widePlacement: "Nodes detail pane",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "View a paired node and check its connection.",
    limitation: "Only connection checks are available in this preview.",
    backPath: "/nodes",
    params: [{ kind: "scalar", name: "nodeId", required: true }],
  },
  {
    id: "nodes.pair",
    path: "/nodes/pair",
    routeFiles: ["app/(shell)/nodes/pair.tsx"],
    label: "Pair a node",
    section: "nodes",
    phonePlacement: "Full-screen flow",
    widePlacement: "Dialog or detail pane",
    deepLink: "none",
    availability: "implementedScaffold",
    summary: "Pair with a nearby node.",
    limitation: "Pairing requires Bluetooth, iOS, and a nearby node.",
    backPath: "/nodes",
    params: [{ kind: "scalar", name: "candidateId", required: false }],
  },
  {
    id: "nodes.grants",
    path: "/nodes/local/grants",
    routeFiles: ["app/(shell)/nodes/local/grants.tsx"],
    label: "Remote access",
    section: "nodes",
    phonePlacement: "Push from local node",
    widePlacement: "Inspector detail pane",
    deepLink: "none",
    availability: "notYetImplemented",
    summary: "Review who can manage this device.",
    limitation: "Remote-access controls are not available yet.",
    backPath: "/nodes/local",
    params: noParams,
  },
  {
    id: "explore.index",
    path: "/explore",
    routeFiles: ["app/(shell)/explore/index.tsx"],
    label: "Explore",
    section: "explore",
    phonePlacement: "Explore tab root",
    widePlacement: "Explore rail group",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "Explore services available through Reticulum.",
    limitation: "NomadNet and location support are coming later.",
    backPath: "/explore",
    params: noParams,
    phoneRootOrder: 3,
    wideRootOrder: 3,
  },
  {
    id: "nomadnet.index",
    path: "/explore/nomadnet",
    routeFiles: ["app/(shell)/explore/nomadnet/index.tsx"],
    label: "NomadNet",
    section: "explore",
    phonePlacement: "Explore stack",
    widePlacement: "NomadNet rail root",
    deepLink: "external-navigation",
    availability: "notYetImplemented",
    summary: "Browse NomadNet pages.",
    limitation: "NomadNet browsing is not available yet.",
    backPath: "/explore",
    params: noParams,
    wideRootOrder: 5,
  },
  {
    id: "nomadnet.page",
    path: "/explore/nomadnet/page",
    routeFiles: ["app/(shell)/explore/nomadnet/page.tsx"],
    label: "NomadNet page",
    section: "explore",
    phonePlacement: "NomadNet detail",
    widePlacement: "NomadNet detail pane",
    deepLink: "external-navigation",
    availability: "notYetImplemented",
    summary: "View a NomadNet page.",
    limitation: "NomadNet pages are not available yet.",
    backPath: "/explore/nomadnet",
    params: [{ kind: "scalar", name: "address", required: true }],
  },
  {
    id: "location.map",
    path: "/explore/location",
    routeFiles: ["app/(shell)/explore/location.tsx"],
    label: "Location",
    section: "explore",
    phonePlacement: "Explore stack",
    widePlacement: "Map rail root",
    deepLink: "app-issued",
    availability: "notYetImplemented",
    summary: "View shared locations and choose when to share yours.",
    limitation: "Location sharing is not available yet.",
    backPath: "/explore",
    params: [
      {
        kind: "union",
        name: "focus",
        required: false,
        literals: ["local"],
        prefixes: ["directory:", "message:"],
      },
    ],
    wideRootOrder: 6,
  },
  {
    id: "more.index",
    path: "/more",
    routeFiles: ["app/(shell)/more/index.tsx"],
    label: "More",
    section: "more",
    phonePlacement: "More tab root",
    widePlacement: "More and settings group",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "Open identities, connections, notifications, settings, and help.",
    limitation: "Some areas are still being built.",
    backPath: "/more",
    params: noParams,
    phoneRootOrder: 4,
    wideRootOrder: 4,
  },
  {
    id: "identities.index",
    path: "/more/identities",
    routeFiles: ["app/(shell)/more/identities/index.tsx"],
    label: "Identities",
    section: "more",
    phonePlacement: "More stack",
    widePlacement: "Identities rail and detail",
    deepLink: "external-navigation",
    availability: "notYetImplemented",
    summary: "View identities used by this app.",
    limitation: "Identity management is not available yet.",
    backPath: "/more",
    params: noParams,
  },
  {
    id: "identities.detail",
    path: "/more/identities/:identityId",
    routeFiles: ["app/(shell)/more/identities/[identityId].tsx"],
    label: "Identity",
    section: "more",
    phonePlacement: "Push",
    widePlacement: "Identities detail pane",
    deepLink: "external-navigation",
    availability: "notYetImplemented",
    summary: "View an identity and what it is used for.",
    limitation: "Identity details are not available yet.",
    backPath: "/more/identities",
    params: [{ kind: "scalar", name: "identityId", required: true }],
  },
  {
    id: "interfaces.index",
    path: "/more/interfaces",
    routeFiles: ["app/(shell)/more/interfaces/index.tsx"],
    label: "Interfaces",
    section: "more",
    phonePlacement: "More stack",
    widePlacement: "Interfaces rail root",
    deepLink: "external-navigation",
    availability: "notYetImplemented",
    summary: "View this device's network connections.",
    limitation: "Connection management is not available yet.",
    backPath: "/more",
    params: noParams,
    wideRootOrder: 7,
  },
  {
    id: "interfaces.detail",
    path: "/more/interfaces/:interfaceId",
    routeFiles: ["app/(shell)/more/interfaces/[interfaceId].tsx"],
    label: "Interface",
    section: "more",
    phonePlacement: "Push",
    widePlacement: "Interfaces detail pane",
    deepLink: "external-navigation",
    availability: "notYetImplemented",
    summary: "View a network connection.",
    limitation: "Connection details are not available yet.",
    backPath: "/more/interfaces",
    params: [{ kind: "scalar", name: "interfaceId", required: true }],
  },
  {
    id: "interfaces.edit",
    path: "/more/interfaces/edit",
    routeFiles: ["app/(shell)/more/interfaces/edit.tsx"],
    label: "Edit interface",
    section: "more",
    phonePlacement: "Full-screen form",
    widePlacement: "Dialog or detail pane",
    deepLink: "none",
    availability: "notYetImplemented",
    summary: "Change a network connection's settings.",
    limitation: "Connection settings cannot be changed yet.",
    backPath: "/more/interfaces",
    params: [{ kind: "scalar", name: "interfaceId", required: false }],
  },
  {
    id: "notifications.settings",
    path: "/more/notifications",
    routeFiles: ["app/(shell)/more/notifications.tsx"],
    label: "Notifications",
    section: "more",
    phonePlacement: "More stack",
    widePlacement: "Settings detail",
    deepLink: "none",
    availability: "notYetImplemented",
    summary: "Choose when prns sends notifications.",
    limitation: "Notifications and background message delivery are not available yet.",
    backPath: "/more",
    params: noParams,
  },
  {
    id: "activity.index",
    path: "/more/activity",
    routeFiles: ["app/(shell)/more/activity/index.tsx"],
    label: "Activity",
    section: "more",
    phonePlacement: "More stack",
    widePlacement: "Activity rail root",
    deepLink: "external-navigation",
    availability: "notYetImplemented",
    summary: "View recent app and network activity.",
    limitation: "Activity history is not available yet.",
    backPath: "/more",
    params: [
      {
        kind: "enum",
        name: "filter",
        required: false,
        values: ["all", "operation", "persistence", "network", "diagnostic"],
      },
    ],
    wideRootOrder: 8,
  },
  {
    id: "activity.operation",
    path: "/more/activity/operation/:operationId",
    routeFiles: ["app/(shell)/more/activity/operation/[operationId].tsx"],
    label: "Operation",
    section: "more",
    phonePlacement: "Push or sheet",
    widePlacement: "Inspector pane",
    deepLink: "app-issued",
    availability: "notYetImplemented",
    summary: "View details for a recent activity.",
    limitation: "Activity details are not available yet.",
    backPath: "/more/activity",
    params: [{ kind: "scalar", name: "operationId", required: true }],
  },
  {
    id: "settings.index",
    path: "/more/settings",
    routeFiles: ["app/(shell)/more/settings/index.tsx"],
    label: "Settings",
    section: "more",
    phonePlacement: "More stack",
    widePlacement: "Settings rail root",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "Choose preview options or reset this app's data.",
    limitation: "These settings affect only this app.",
    backPath: "/more",
    params: noParams,
  },
  {
    id: "settings.storage",
    path: "/more/settings/storage",
    routeFiles: ["app/(shell)/more/settings/storage.tsx"],
    label: "Storage",
    section: "more",
    phonePlacement: "Settings stack",
    widePlacement: "Settings detail",
    deepLink: "none",
    availability: "notYetImplemented",
    summary: "See how much space prns uses on this device.",
    limitation: "Storage details are not available yet.",
    backPath: "/more/settings",
    params: noParams,
  },
  {
    id: "settings.recovery",
    path: "/more/settings/recovery",
    routeFiles: ["app/(shell)/more/settings/recovery.tsx"],
    label: "Reset and recovery",
    section: "more",
    phonePlacement: "Settings stack",
    widePlacement: "Settings detail",
    deepLink: "none",
    availability: "notYetImplemented",
    summary: "Reset or recover app data.",
    limitation: "Recovery without resetting is not available yet.",
    backPath: "/more/settings",
    params: [
      {
        kind: "enum",
        name: "resetScope",
        required: false,
        values: ["preferences", "service-data", "identity", "installation"],
      },
    ],
  },
  {
    id: "settings.about",
    path: "/more/settings/about",
    routeFiles: ["app/(shell)/more/settings/about.tsx"],
    label: "About",
    section: "more",
    phonePlacement: "Settings stack",
    widePlacement: "Settings detail",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "View app and version information.",
    limitation: "This is a development preview, not a release build.",
    backPath: "/more/settings",
    params: noParams,
  },
  {
    id: "help.index",
    path: "/more/help",
    routeFiles: ["app/(shell)/more/help/index.tsx"],
    label: "Help",
    section: "more",
    phonePlacement: "More stack",
    widePlacement: "Help detail",
    deepLink: "external-navigation",
    availability: "implementedScaffold",
    summary: "See what works in this preview and what is coming later.",
    limitation: "Preview data may be reset and cannot be backed up yet.",
    backPath: "/more",
    params: noParams,
  },
] as const satisfies readonly ScreenCatalogEntry[];

export type ScreenId = (typeof screenCatalog)[number]["id"];
export type RawRouteParams = Readonly<Record<string, string | string[] | undefined>>;

export function screenById(id: ScreenId): (typeof screenCatalog)[number] {
  const screen = screenCatalog.find((candidate) => candidate.id === id);
  if (screen === undefined) {
    throw new Error(`screen catalog entry is missing: ${id}`);
  }
  return screen;
}

function isNonEmptySingle(value: string | string[] | undefined): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function valueMatches(rule: RouteParamRule, value: string): boolean {
  if (rule.kind === "scalar") {
    return rule.format === "destinationHash" ? /^[0-9a-fA-F]{32}$/u.test(value) : true;
  }
  if (rule.kind === "enum") {
    return rule.values.includes(value);
  }
  return (
    (rule.literals?.includes(value) ?? false) ||
    rule.prefixes.some((prefix) => value.startsWith(prefix) && value.length > prefix.length)
  );
}

export function routeParamsAreValid(entry: ScreenCatalogEntry, params: RawRouteParams): boolean {
  const allowed = new Set(entry.params.map((rule) => rule.name));
  for (const [name, value] of Object.entries(params)) {
    if (!allowed.has(name) || Array.isArray(value)) {
      return false;
    }
  }

  return entry.params.every((rule) => {
    const value = params[rule.name];
    if (value === undefined) {
      return !rule.required;
    }
    return isNonEmptySingle(value) && valueMatches(rule, value);
  });
}

export function navigationEntries(
  layout: "phone" | "wide",
  showUnavailableFeatures: boolean,
): readonly ScreenCatalogEntry[] {
  const orderFor = (entry: ScreenCatalogEntry): number | undefined =>
    layout === "phone" ? entry.phoneRootOrder : entry.wideRootOrder;
  return screenCatalog
    .filter((entry) => orderFor(entry) !== undefined)
    .filter((entry) => showUnavailableFeatures || entry.availability === "implementedScaffold")
    .sort((left, right) => (orderFor(left) ?? 0) - (orderFor(right) ?? 0));
}
