import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onMount,
} from "solid-js";
import {
  Tag,
  match,
  match_into,
} from "personal-rns/browser";
import type { Tag as Tagged } from "personal-rns/browser";
import type { PrnsDrop } from "../app/drop.js";
import type {
  DropContact,
  DropMessage,
  DropOutboundState,
  DropSendFailure,
  DropSnapshot,
} from "../app/model.js";
import {
  MAX_DROP_TEXT_BYTES,
  exportDropContactCode,
} from "../app/protocol.js";
import { createDropSnapshot } from "./adapter.js";

type Notice =
  | Tagged<"Idle">
  | Tagged<"Success", { readonly text: string }>
  | Tagged<"Failure", { readonly text: string }>;

export function DropApp(props: { readonly drop: PrnsDrop }) {
  const snapshot = createDropSnapshot(props.drop);
  const [selectedDestination, setSelectedDestination] = createSignal<string>();
  const [draft, setDraft] = createSignal("");
  const [contactInput, setContactInput] = createSignal("");
  const [notice, setNotice] = createSignal<Notice>(Tag("Idle"));
  const [sending, setSending] = createSignal(false);
  const selectedContact = createMemo(() =>
    snapshot().contacts.find(({ destinationHex }) =>
      destinationHex === selectedDestination()
    )
  );
  const conversation = createMemo(() => {
    const selected = selectedDestination();
    return selected === undefined
      ? []
      : snapshot().messages.filter((message) =>
          message.data.peerDestinationHex === selected
        );
  });
  const textBytes = createMemo(() => new TextEncoder().encode(draft()).length);
  const noticeText = createMemo(() => {
    const current = notice();
    return current.tag === "Idle" ? "" : current.data.text;
  });
  const activeGateways = createMemo(() => {
    const transport = snapshot().transport;
    return transport.tag === "Active" ? transport.data.gateways : [];
  });

  createEffect(() => {
    const contacts = snapshot().contacts;
    const selected = selectedDestination();
    if (selected !== undefined && contacts.some(({ destinationHex }) => destinationHex === selected)) {
      return;
    }
    setSelectedDestination(contacts[0]?.destinationHex);
  });

  onMount(() => {
    const shared = contactCodeFromLocation();
    if (shared !== undefined) {
      setContactInput(shared);
      void importContact(shared, true);
    }
  });

  async function importContact(code: string = contactInput(), fromLink = false): Promise<void> {
    const outcome = await props.drop.importContact(code);
    match(outcome, {
      Imported: ({ contact, persistence }) => {
        setSelectedDestination(contact.destinationHex);
        setContactInput("");
        setNotice(persistence.tag === "Saved"
          ? Tag("Success", { text: `${contact.advertisedName} was saved.` })
          : Tag("Failure", {
              text: `${contact.advertisedName} is available for this session only: ${persistence.data.detail}`,
            }));
        if (fromLink) {
          globalThis.history.replaceState(null, "", `${globalThis.location.pathname}${globalThis.location.search}`);
        }
      },
      InvalidContactCode: ({ detail }) => {
        setNotice(Tag("Failure", { text: detail }));
      },
      SelfContact: () => {
        setNotice(Tag("Failure", { text: "That contact code belongs to this Drop." }));
      },
    });
  }

  async function saveContact(contact: DropContact): Promise<void> {
    await importContact(exportDropContactCode({
      destinationHex: contact.destinationHex,
      displayName: contact.advertisedName,
    }));
  }

  async function forgetContact(contact: DropContact): Promise<void> {
    const outcome = await props.drop.forgetContact(contact.destinationHex);
    match(outcome, {
      Forgotten: () => setNotice(Tag("Success", {
        text: `${contact.advertisedName} was removed from saved contacts.`,
      })),
      NotSaved: () => setNotice(Tag("Failure", {
        text: `${contact.advertisedName} is not a saved contact.`,
      })),
      StorageUnavailable: ({ detail }) => setNotice(Tag("Failure", { text: detail })),
    });
  }

  async function sendMessage(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const contact = selectedContact();
    if (contact === undefined || sending()) {
      return;
    }
    const text = draft();
    setSending(true);
    setDraft("");
    const outcome = await props.drop.sendText(contact.destinationHex, text);
    match(outcome, {
      Delivered: ({ rttMillis }) => setNotice(Tag("Success", {
        text: `Delivered to ${contact.advertisedName} in ${rttMillis} ms.`,
      })),
      Rejected: (failure) => {
        setDraft(text);
        setNotice(Tag("Failure", { text: describeSendFailure(failure) }));
      },
    });
    setSending(false);
  }

  async function copyContact(): Promise<void> {
    try {
      await globalThis.navigator.clipboard.writeText(snapshot().identity.contactCode);
      setNotice(Tag("Success", { text: "Contact code copied." }));
    } catch (error: unknown) {
      setNotice(Tag("Failure", { text: describeUnknown(error) }));
    }
  }

  async function shareContact(): Promise<void> {
    const url = contactUrl(snapshot());
    if (globalThis.navigator.share === undefined) {
      await copyContact();
      return;
    }
    try {
      await globalThis.navigator.share({
        title: `${snapshot().identity.displayName} on Prns Drop`,
        text: snapshot().identity.contactCode,
        url,
      });
      setNotice(Tag("Success", { text: "Contact shared." }));
    } catch (error: unknown) {
      if (error instanceof DOMException && error.name === "AbortError") {
        return;
      }
      setNotice(Tag("Failure", { text: describeUnknown(error) }));
    }
  }

  return (
    <main class="drop-shell">
      <header class="topbar">
        <a class="brand" href="/">
          <span class="brand-mark">P</span>
          <span>Prns Drop</span>
        </a>
        <div class="transport-pill" data-state={transportTone(snapshot())}>
          <span />
          {transportLabel(snapshot())}
        </div>
      </header>

      <section class="identity-card">
        <div>
          <p class="eyebrow">Your persistent Drop</p>
          <h1>{snapshot().identity.displayName}</h1>
          <code>{snapshot().identity.destinationHex}</code>
        </div>
        <div class="identity-actions">
          <button class="primary" onClick={() => void shareContact()}>Share contact</button>
          <button onClick={() => void copyContact()}>Copy code</button>
          <button onClick={() => void props.drop.announce()}>Announce now</button>
        </div>
      </section>

      <Show when={notice().tag !== "Idle"}>
        <div class={`notice ${notice().tag.toLowerCase()}`}>
          {noticeText()}
          <button aria-label="Dismiss" onClick={() => setNotice(Tag("Idle"))}>×</button>
        </div>
      </Show>

      <div class="app-grid">
        <aside class="contacts-panel">
          <div class="panel-heading">
            <div>
              <p class="eyebrow">People</p>
              <h2>Contacts</h2>
            </div>
            <span class="count">{snapshot().contacts.length}</span>
          </div>

          <form class="contact-import" onSubmit={(event) => {
            event.preventDefault();
            void importContact();
          }}>
            <input
              aria-label="Prns Drop contact code"
              onInput={(event) => setContactInput(event.currentTarget.value)}
              placeholder="Paste a prns-drop contact code"
              value={contactInput()}
            />
            <button disabled={contactInput().trim().length === 0}>Add</button>
          </form>

          <Show
            when={snapshot().contacts.length > 0}
            fallback={
              <div class="empty-state">
                <strong>No contacts yet</strong>
                <p>Share your code, paste somebody else’s, or wait for a nearby Drop announce.</p>
              </div>
            }
          >
            <div class="contact-list">
              <For each={snapshot().contacts}>
                {(contact) => (
                  <article
                    class="contact"
                    classList={{ selected: selectedDestination() === contact.destinationHex }}
                  >
                    <button class="contact-select" onClick={() => setSelectedDestination(contact.destinationHex)}>
                      <span class="avatar">{initials(contact.advertisedName)}</span>
                      <span class="contact-copy">
                        <strong>{contact.advertisedName}</strong>
                        <small>{shortHash(contact.destinationHex)}</small>
                      </span>
                      <span
                        class="reachability"
                        classList={{ nearby: contact.reachability.tag === "Announced" }}
                        title={reachabilityLabel(contact)}
                      />
                    </button>
                    <div class="contact-actions">
                      <Show
                        when={contact.persistence.tag === "Saved"}
                        fallback={<button onClick={() => void saveContact(contact)}>Save</button>}
                      >
                        <button onClick={() => void forgetContact(contact)}>Forget</button>
                      </Show>
                    </div>
                  </article>
                )}
              </For>
            </div>
          </Show>
        </aside>

        <section class="conversation-panel">
          <Show
            when={selectedContact()}
            keyed
            fallback={
              <div class="conversation-empty">
                <div class="empty-glyph">↗</div>
                <h2>Choose somebody to Drop to</h2>
                <p>Saved contacts work whenever Prns can discover a path. Nearby contacts appeared from live announces.</p>
              </div>
            }
          >
            {(contact) => (
              <>
                <header class="conversation-heading">
                  <div>
                    <p class="eyebrow">Conversation</p>
                    <h2>{contact.advertisedName}</h2>
                  </div>
                  <div class="peer-state">
                    {contact.persistence.tag === "Saved" ? "saved" : "session only"}
                    <span>·</span>
                    {reachabilityLabel(contact)}
                  </div>
                </header>

                <div class="messages" aria-live="polite">
                  <Show
                    when={conversation().length > 0}
                    fallback={
                      <div class="message-placeholder">
                        <span>Say hello through the actual Prns network.</span>
                      </div>
                    }
                  >
                    <For each={conversation()}>
                      {(message) => <MessageBubble message={message} />}
                    </For>
                  </Show>
                </div>

                <form class="composer" onSubmit={(event) => void sendMessage(event)}>
                  <textarea
                    aria-label={`Message ${contact.advertisedName}`}
                    onInput={(event) => setDraft(event.currentTarget.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" && !event.shiftKey) {
                        event.preventDefault();
                        event.currentTarget.form?.requestSubmit();
                      }
                    }}
                    placeholder={`Message ${contact.advertisedName}`}
                    value={draft()}
                  />
                  <div class="composer-footer">
                    <span classList={{ over: textBytes() > MAX_DROP_TEXT_BYTES }}>
                      {textBytes()} / {MAX_DROP_TEXT_BYTES} bytes
                    </span>
                    <button
                      class="primary"
                      disabled={sending() || draft().trim().length === 0 || textBytes() > MAX_DROP_TEXT_BYTES}
                    >
                      {sending() ? "Sending…" : "Send"}
                    </button>
                  </div>
                </form>
              </>
            )}
          </Show>
        </section>
      </div>

      <details class="diagnostics">
        <summary>Network and application state</summary>
        <dl>
          <dt>Auto Wi-Fi</dt><dd>{snapshot().transport.tag}</dd>
          <dt>Announcement</dt><dd>{snapshot().announcement.tag}</dd>
          <dt>Discovery stream</dt><dd>{snapshot().discovery.tag}</dd>
          <dt>Contact storage</dt><dd>{snapshot().storage.tag}</dd>
          <dt>Application stream</dt><dd>{snapshot().health.tag}</dd>
          <dt>Lifecycle</dt><dd>{snapshot().lifecycle.tag}</dd>
        </dl>
        <Show when={snapshot().transport.tag === "Active"}>
          <div class="gateway-chips">
            <For each={activeGateways()}>
              {(gateway) => <code>{gateway.url}</code>}
            </For>
          </div>
        </Show>
      </details>
    </main>
  );
}

function MessageBubble(props: { readonly message: DropMessage }) {
  return (
    <article class={`message ${props.message.tag.toLowerCase()}`}>
      <p>{props.message.data.text}</p>
      <footer>
        <time>{new Date(props.message.data.sentAt).toLocaleTimeString([], {
          hour: "numeric",
          minute: "2-digit",
        })}</time>
        <Show when={props.message.tag === "Outbound"}>
          <span>{props.message.tag === "Outbound"
            ? outboundStateLabel(props.message.data.state)
            : ""}</span>
        </Show>
      </footer>
    </article>
  );
}

function outboundStateLabel(state: DropOutboundState): string {
  return match_into<string>().from(state, {
    Sending: () => "sending",
    DiscoveringPath: () => "finding path",
    Delivered: ({ rttMillis }) => `delivered · ${rttMillis} ms`,
    Failed: (failure) => `failed · ${describeSendFailure(failure)}`,
  });
}

function describeSendFailure(failure: DropSendFailure): string {
  return match_into<string>().from(failure, {
    EmptyText: () => "Write something before sending.",
    TextTooLong: ({ actualBytes, maximumBytes }) =>
      `${actualBytes} bytes exceeds the ${maximumBytes}-byte text limit.`,
    UnknownContact: () => "That contact is no longer available.",
    SelfDelivery: () => "A Drop cannot send to itself.",
    EntropyUnavailable: ({ detail }) => detail,
    SendRejected: ({ failure: commandFailure }) => commandFailureLabel(commandFailure.tag),
    PathDiscoveryRejected: ({ failure: commandFailure }) =>
      `Path discovery failed: ${commandFailureLabel(commandFailure.tag)}`,
    RetryRejected: ({ failure: commandFailure }) =>
      `Delivery failed after path discovery: ${commandFailureLabel(commandFailure.tag)}`,
    UnexpectedFailure: ({ detail }) => detail,
    Closed: () => "This Drop is closed.",
  });
}

function commandFailureLabel(tag: string): string {
  return tag.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase();
}

function transportTone(snapshot: DropSnapshot): string {
  return match_into<string>().from(snapshot.transport, {
    Starting: () => "working",
    Discovering: () => "working",
    Active: () => "active",
    Unavailable: () => "failed",
    Closed: () => "closed",
  });
}

function transportLabel(snapshot: DropSnapshot): string {
  return match_into<string>().from(snapshot.transport, {
    Starting: () => "Starting Auto Wi-Fi",
    Discovering: ({ attempt }) => `Discovering · attempt ${attempt}`,
    Active: ({ gateways }) => `${gateways.length} transport${gateways.length === 1 ? "" : "s"} active`,
    Unavailable: () => "Transport unavailable",
    Closed: () => "Transport closed",
  });
}

function reachabilityLabel(contact: DropContact): string {
  return contact.reachability.tag === "Announced"
    ? `nearby · ${contact.reachability.data.hops} hop${contact.reachability.data.hops === 1 ? "" : "s"}`
    : "not recently announced";
}

function contactUrl(snapshot: DropSnapshot): string {
  const url = new URL(globalThis.location.href);
  url.hash = `contact=${encodeURIComponent(snapshot.identity.contactCode)}`;
  return url.href;
}

function contactCodeFromLocation(): string | undefined {
  if (!globalThis.location.hash.startsWith("#contact=")) {
    return undefined;
  }
  try {
    return decodeURIComponent(globalThis.location.hash.slice("#contact=".length));
  } catch {
    return undefined;
  }
}

function initials(name: string): string {
  return name
    .split(/\s+/)
    .filter((part) => part.length > 0)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase() ?? "")
    .join("");
}

function shortHash(value: string): string {
  return `${value.slice(0, 8)}…${value.slice(-6)}`;
}

function describeUnknown(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
