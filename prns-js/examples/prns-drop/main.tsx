import {
  Show,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";
import { render } from "solid-js/web";
import {
  Prns,
  Tag,
  persistentBrowser,
} from "personal-rns/browser";
import type {
  PrnsCreateOutcome,
  Tag as Tagged,
} from "personal-rns/browser";
import {
  PrnsProvider,
  usePrns,
} from "personal-rns/solid";
import { BrowserDropContactStore } from "./app/contact_store.js";
import { PrnsDrop } from "./app/drop.js";
import type { DropOpenOutcome } from "./app/drop.js";
import { DropApp } from "./solid/DropApp.js";
import "./styles.css";

const PERSISTENCE_ROOT = "prns.drop";
const DISPLAY_NAME_KEY = `${PERSISTENCE_ROOT}.display-name.v1`;

type BootstrapState =
  | Tagged<"Opening">
  | Tagged<"Ready", PrnsDrop>
  | Tagged<"Failed", { readonly detail: string }>;

function DropBootstrap() {
  const prns = usePrns();
  const [state, setState] = createSignal<BootstrapState>(Tag("Opening"));
  const readyDrop = () => {
    const current = state();
    return current.tag === "Ready" ? current.data : undefined;
  };
  const startupTitle = () =>
    state().tag === "Failed" ? "Prns Drop could not start." : "Opening your Drop…";
  const startupDetail = () => {
    const current = state();
    return current.tag === "Failed" ? current.data.detail : undefined;
  };

  onMount(() => {
    let active = true;
    void PrnsDrop.open(prns, {
      displayName: loadOrCreateDisplayName(),
      contactStore: new BrowserDropContactStore(PERSISTENCE_ROOT),
    }).then((outcome) => {
      if (!active) {
        if (outcome.tag === "Opened") {
          void outcome.data.close();
        }
        return;
      }
      setState(outcome.tag === "Opened"
        ? Tag("Ready", outcome.data)
        : Tag("Failed", { detail: describeDropOpenFailure(outcome) }));
    }).catch((error: unknown) => {
      if (active) {
        setState(Tag("Failed", { detail: describeUnknown(error) }));
      }
    });
    onCleanup(() => {
      active = false;
      const current = state();
      if (current.tag === "Ready") {
        void current.data.close();
      }
    });
  });

  return (
    <Show
      when={readyDrop()}
      keyed
      fallback={
        <main class="startup">
          <p class="eyebrow">personal-rns × SolidJS</p>
          <h1>{startupTitle()}</h1>
          <Show when={startupDetail()} keyed>
            {(detail) => <pre><code>{detail}</code></pre>}
          </Show>
        </main>
      }
    >
      {(drop) => <DropApp drop={drop} />}
    </Show>
  );
}

function loadOrCreateDisplayName(): string {
  const stored = globalThis.localStorage.getItem(DISPLAY_NAME_KEY)?.trim();
  if (stored !== undefined && stored.length > 0) {
    return stored;
  }
  const suffix = new Uint8Array(2);
  globalThis.crypto.getRandomValues(suffix);
  const displayName = `Drop ${[...suffix]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("")}`;
  globalThis.localStorage.setItem(DISPLAY_NAME_KEY, displayName);
  return displayName;
}

function describeDropOpenFailure(
  outcome: Exclude<DropOpenOutcome, { readonly tag: "Opened" }>,
): string {
  if (outcome.tag === "DisplayNameEmpty") {
    return "The Drop display name is empty.";
  }
  if (outcome.tag === "DisplayNameTooLong") {
    return `The Drop display name is ${outcome.data.actualBytes} bytes; maximum ${outcome.data.maximumBytes}.`;
  }
  if (outcome.tag === "ApplicationEventsUnavailable") {
    return `${outcome.data.lane} already has a consumer.`;
  }
  return `${outcome.data.operation}: ${outcome.data.detail}`;
}

function describeStartupFailure(
  outcome: Exclude<PrnsCreateOutcome, { readonly tag: "Ready" }>,
): string {
  return `${outcome.tag}\n${JSON.stringify(outcome.data, jsonReplacer, 2)}`;
}

function jsonReplacer(_key: string, value: unknown): unknown {
  return typeof value === "bigint" ? value.toString() : value;
}

function describeUnknown(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function FailurePage(props: { readonly title: string; readonly detail: string }) {
  return (
    <main class="startup failure-page">
      <p class="eyebrow">personal-rns × SolidJS</p>
      <h1>{props.title}</h1>
      <pre><code>{props.detail}</code></pre>
    </main>
  );
}

async function start(): Promise<void> {
  const root = document.getElementById("root");
  if (root === null) {
    throw new Error("Prns Drop root element is missing");
  }
  const wasmModuleUrl = new URL("/prns_wasm.js", globalThis.location.href);
  const created = await Prns.create({
    ...persistentBrowser(PERSISTENCE_ROOT),
    execution: "DedicatedWorker",
    wasmModuleUrl,
    resourceCompressionModuleUrl: wasmModuleUrl,
  });
  root.replaceChildren();
  if (created.tag !== "Ready") {
    render(() => (
      <FailurePage
        title="Prns could not start."
        detail={describeStartupFailure(created)}
      />
    ), root);
    return;
  }
  const prns = created.data;
  const dispose = render(
    () => (
      <PrnsProvider prns={prns}>
        <DropBootstrap />
      </PrnsProvider>
    ),
    root,
  );
  globalThis.addEventListener("pagehide", () => {
    dispose();
    void prns.stop();
  }, { once: true });
}

void start().catch((error: unknown) => {
  const root = document.getElementById("root");
  if (root !== null) {
    root.replaceChildren();
    render(() => (
      <FailurePage
        title="Prns Drop startup crashed."
        detail={describeUnknown(error)}
      />
    ), root);
  }
});
