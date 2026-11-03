import { Link, useLocalSearchParams, usePathname, useRouter } from "expo-router";
import { useCallback, useEffect, useRef } from "react";
import { IdentityPanel } from "../IdentityPanel";
import type { IdentitySection } from "../IdentityPanel";
import { Identities } from "../Identities";
import { contactKey } from "../contacts";
import { identityKey } from "../identity-reference";
import { scalar, useRouteLease } from "../navigation";
import { useInstallation } from "../runtime/InstallationProvider";
import { Button, Heading, PageHeader, Screen, Text } from "../ui/primitives";

export function IdentityScreen({ section }: { section: IdentitySection }) {
  const installation = useInstallation();
  const params = useLocalSearchParams<{ owner?: string; reference?: string; group?: string; contact?: string; invitation?: string; incarnation?: string; message?: string }>();
  const requested = scalar(params.reference) ?? scalar(params.owner) ?? installation.selected;
  const record = installation.records.find((value) => identityKey(value) === requested);
  useEffect(() => {
    if (requested !== undefined && record !== undefined && installation.selectionReady) installation.select(requested);
  }, [requested, record?.id.sequence, installation.selectionReady, installation.select]);
  const router = useRouter();
  const pathname = usePathname();
  const { active, isCurrent } = useRouteLease();
  const recipient = scalar(params.contact);
  const scope = `${pathname}:${requested ?? ""}:${recipient ?? ""}:${scalar(params.incarnation) ?? ""}:${scalar(params.message) ?? ""}`;
  const owner = installation.owner;
  const currentOwner = useRef(owner); currentOwner.current = owner;
  const currentScope = useRef(scope);
  currentScope.current = scope;
  const canNavigate = useCallback(() => isCurrent() && currentScope.current === scope && currentOwner.current === owner, [isCurrent, scope, owner]);
  const openGroup = useCallback((group: string) => {
    if (requested !== undefined && canNavigate()) router.push({ pathname: "/conversations/group/[group]", params: { group, owner: requested, ...(recipient === undefined ? {} : { contact: recipient }) } });
  }, [requested, recipient, router, canNavigate]);
  const openInviteGroup = useCallback((group: string) => {
    if (requested !== undefined && canNavigate()) router.push({ pathname: "/conversations/group/[group]/invite", params: { group, owner: requested, ...(recipient === undefined ? {} : { contact: recipient }) } });
  }, [requested, recipient, router, canNavigate]);
  const openDetails = useCallback((group: string) => {
    if (requested !== undefined && canNavigate()) router.push({ pathname: "/conversations/group/[group]/details", params: { group, owner: requested } });
  }, [requested, router, canNavigate]);
  const openDelivery = useCallback((group: string) => {
    if (requested !== undefined && canNavigate()) router.push({ pathname: "/conversations/group/[group]/delivery", params: { group, owner: requested } });
  }, [requested, router, canNavigate]);
  const openMessage = useCallback((group: string, incarnation: string, message: string) => {
    if (requested !== undefined && canNavigate()) router.push({ pathname: "/conversations/group/[group]/message/[incarnation]/[message]", params: { group, incarnation, message, owner: requested } });
  }, [requested, router, canNavigate]);
  const openInvitation = useCallback((invitation: string) => {
    if (requested !== undefined && canNavigate()) router.push({ pathname: "/conversations/invitation/[invitation]", params: { invitation, owner: requested } });
  }, [requested, router, canNavigate]);
  const openActivity = useCallback(() => {
    if (requested !== undefined && canNavigate()) router.push({ pathname: "/conversations/activity", params: { owner: requested } });
  }, [requested, router, canNavigate]);
  const openCreate = useCallback(() => {
    if (requested !== undefined && canNavigate()) router.push({ pathname: "/conversations/new", params: { owner: requested } });
  }, [requested, router, canNavigate]);
  const group = scalar(params.group);
  const invitation = scalar(params.invitation);
  const incarnation = scalar(params.incarnation), messageId = scalar(params.message);
  const messageRoute = /^\/conversations\/group\/[^/]+\/message\/[^/]+\/[^/]+$/.test(pathname);
  const messagingScreen = messageRoute ? "message" : invitation !== undefined ? "invitation" : pathname === "/conversations/activity" ? "activity"
    : pathname.endsWith("/invite") && group !== undefined ? "invite" : pathname.endsWith("/details") && group !== undefined ? "details" : pathname.endsWith("/delivery") && group !== undefined ? "delivery" : pathname === "/conversations/new" ? "create" : group === undefined ? "list" : "conversation";
  if ((params.owner !== undefined && scalar(params.owner) === undefined) ||
      (params.reference !== undefined && scalar(params.reference) === undefined) ||
      (params.reference !== undefined && params.owner !== undefined && scalar(params.reference) !== scalar(params.owner)) ||
      ((params.group !== undefined || params.invitation !== undefined || params.incarnation !== undefined || params.message !== undefined || messageRoute || pathname === "/conversations/activity") && params.owner === undefined)) {
    return <Screen><Heading>Identity unavailable</Heading><Text>Choose a messaging identity from Conversations to open this page.</Text></Screen>;
  }
  if (params.group !== undefined && (group === undefined || !/^[0-9a-f]{64}$/.test(group))) {
    return <Screen><Heading>Conversation unavailable</Heading><Text>The conversation reference is invalid.</Text></Screen>;
  }
  if ((messageRoute || params.incarnation !== undefined || params.message !== undefined) &&
      (!messageRoute || group === undefined || incarnation === undefined || !/^[0-9a-f]{48}$/.test(incarnation) || messageId === undefined || !/^[0-9a-f]{64}$/.test(messageId))) {
    return <Screen><Heading>Message unavailable</Heading><Text>The message reference is invalid. Open a message from its conversation.</Text></Screen>;
  }
  if (params.invitation !== undefined && (invitation === undefined || !/^[0-9a-f]{64}$/.test(invitation))) {
    return <Screen><Heading>Invitation unavailable</Heading><Text>The invitation reference is invalid.</Text></Screen>;
  }
  if (params.contact !== undefined && (recipient === undefined || !/^[0-9a-f]{32}:[0-9a-f]{20}$/.test(recipient) || params.owner === undefined || section !== "Conversations")) {
    return <Screen><Heading>Contact unavailable</Heading><Text>Choose a saved contact from this messaging identity.</Text></Screen>;
  }
  return <Screen>
    {section === "Identity" && <PageHeader title="Identity settings" help={{ label: "About messaging identities", content: <Text>Each identity has its own contacts and conversations. Your private label stays on this phone; your shared name appears on contact cards and announcements.</Text> }} />}
    {section !== "Identity" && record === undefined && <PageHeader title={section} />}
    {installation.catalogError !== undefined && <Text accessibilityRole="alert">{installation.catalogError}</Text>}
    {installation.owner === undefined ? <><Text>Open this phone’s saved installation to use your identities.</Text>
      <Link href="/setup" asChild><Button title="Open phone setup" /></Link></>
      : record === undefined ? <><Text>{requested === undefined ? "Create or choose an identity to continue." : "This identity is not available in this installation. The route has not been changed to another identity."}</Text>
        <Link href="/more/identities" asChild><Button title="Choose or create an identity" /></Link></>
      : <IdentityPanel key={identityKey(record)} owner={installation.owner} record={record} active={active} section={section}
        group={group} recipient={recipient} onOpenGroup={openGroup} messagingScreen={messagingScreen} invitation={invitation} onOpenInvitation={openInvitation} onInviteGroup={openInviteGroup} onOpenDetails={openDetails} onOpenDelivery={openDelivery} incarnation={incarnation} messageId={messageId} onOpenMessage={openMessage} onOpenActivity={openActivity} onOpenCreate={openCreate}
        onChanged={installation.reloadIdentities} contactsRevision={installation.contactsRevision} onContactsChanged={installation.contactSaved}
        onSectionChange={(next) => { if (canNavigate()) router.push({ pathname: next === "Identity" ? "/more/identities/[reference]" : next === "Contacts" ? "/contacts" : "/conversations", params: { reference: requested!, owner: requested! } }); }}
        onOpenContact={(contact) => { if (canNavigate()) router.push({ pathname: "/contacts/[contact]", params: { owner: requested!, contact: contactKey(contact) } }); }} />}
  </Screen>;
}

export function IdentitiesScreen() {
  const installation = useInstallation();
  const router = useRouter();
  const { active, isCurrent } = useRouteLease();
  return <Screen><PageHeader title="Messaging identities" help={{ label: "About messaging identities", content: <Text>Use separate identities for different parts of your life. Each keeps its own contacts and conversations.</Text> }} />
    {installation.owner === undefined ? <><Text>Open your installation to create or choose an identity.</Text>
      <Link href="/setup" asChild><Button title="Open phone setup" /></Link></>
      : <Identities owner={installation.owner} active={active} onSelect={(reference) => {
        if (isCurrent()) { installation.select(reference); router.push({ pathname: "/more/identities/[reference]", params: { reference } }); }
      }} />}
  </Screen>;
}
