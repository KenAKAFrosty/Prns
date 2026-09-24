import { render } from "@testing-library/react-native";
import type { ComponentType } from "react";
import ContactRoute from "../../app/(shell)/contacts/[destination]";
import AddContactRoute from "../../app/(shell)/contacts/add";
import ContactsRoute from "../../app/(shell)/contacts/index";
import ExploreRoute from "../../app/(shell)/explore/index";
import ComposeRoute from "../../app/(shell)/inbox/compose";
import ConversationRoute from "../../app/(shell)/inbox/conversation/[destination]";
import InboxRoute from "../../app/(shell)/inbox/index";
import HelpRoute from "../../app/(shell)/more/help/index";
import MoreRoute from "../../app/(shell)/more/index";
import AboutRoute from "../../app/(shell)/more/settings/about";
import SettingsRoute from "../../app/(shell)/more/settings/index";
import NodesRoute from "../../app/(shell)/nodes/index";
import LocalNodeRoute from "../../app/(shell)/nodes/local/index";
import OnboardingRoute from "../../app/onboarding/index";
import RecoveryRoute from "../../app/recovery";
import type { RawRouteParams } from "./catalog";

const mockUseLocalSearchParams = jest.fn<RawRouteParams, []>();

jest.mock("expo-router", () => ({
  useLocalSearchParams: () => mockUseLocalSearchParams(),
  useRouter: () => ({ replace: jest.fn() }),
}));

jest.mock("@/features/onboarding/onboarding-screen", () => ({
  OnboardingScreen: () => null,
}));

jest.mock("@/features/onboarding/recovery-screen", () => ({
  RecoveryScreen: () => null,
}));

jest.mock("@/features/inbox/inbox-screen", () => ({
  ComposeScreen: () => null,
  ConversationScreen: () => null,
  InboxScreen: () => null,
}));

jest.mock("@/features/contacts/contacts-screen", () => ({
  AddContactScreen: () => null,
  ContactDetailScreen: () => null,
  ContactsScreen: () => null,
}));

jest.mock("@/features/nodes/nodes-screen", () => ({
  LocalNodeScreen: () => null,
  NodesScreen: () => null,
}));

jest.mock("@/features/explore/explore-screen", () => ({
  ExploreScreen: () => null,
}));

jest.mock("@/features/more/more-screen", () => ({
  MoreScreen: () => null,
}));

jest.mock("@/features/settings/settings-screen", () => ({
  SettingsScreen: () => null,
}));

jest.mock("@/features/settings/about-screen", () => ({
  AboutScreen: () => null,
}));

jest.mock("@/features/settings/help-screen", () => ({
  HelpScreen: () => null,
}));

type ImplementedRouteCase = {
  readonly label: string;
  readonly Component: ComponentType;
  readonly validParams: RawRouteParams;
};

const destination = "00".repeat(16);
const implementedRoutes: readonly ImplementedRouteCase[] = [
  { label: "onboarding", Component: OnboardingRoute, validParams: {} },
  {
    label: "recovery",
    Component: RecoveryRoute,
    validParams: { installationId: "current" },
  },
  { label: "inbox", Component: InboxRoute, validParams: {} },
  {
    label: "conversation",
    Component: ConversationRoute,
    validParams: { destination },
  },
  { label: "compose", Component: ComposeRoute, validParams: { destination } },
  { label: "contacts", Component: ContactsRoute, validParams: {} },
  { label: "contact", Component: ContactRoute, validParams: { destination } },
  { label: "add contact", Component: AddContactRoute, validParams: {} },
  { label: "nodes", Component: NodesRoute, validParams: {} },
  { label: "this device", Component: LocalNodeRoute, validParams: {} },
  { label: "explore", Component: ExploreRoute, validParams: {} },
  { label: "more", Component: MoreRoute, validParams: {} },
  { label: "settings", Component: SettingsRoute, validParams: {} },
  { label: "about", Component: AboutRoute, validParams: {} },
  { label: "help", Component: HelpRoute, validParams: {} },
];

describe("implemented route parameter guards", () => {
  it.each(implementedRoutes)("rejects unknown $label parameters", ({ Component, validParams }) => {
    mockUseLocalSearchParams.mockReturnValue({ ...validParams, unexpected: "value" });

    const view = render(<Component />);

    expect(view.getByText("Not found")).toBeTruthy();
  });

  it.each(implementedRoutes)("accepts declared $label parameters", ({ Component, validParams }) => {
    mockUseLocalSearchParams.mockReturnValue(validParams);

    const view = render(<Component />);

    expect(view.queryByText("Not found")).toBeNull();
  });
});
