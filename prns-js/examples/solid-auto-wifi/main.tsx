import {
  For,
  Show,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";
import { render } from "solid-js/web";
import {
  Prns,
  Tag,
  match,
  match_into,
  persistentBrowser,
} from "personal-rns/browser";
import type {
  AutoWifiControllerStatus,
  AutoWifiFailure,
  AutoWifiGatewayStatus,
  PrnsCreateOutcome,
  PrnsSnapshot,
  Tag as Tagged,
} from "personal-rns/browser";
import {
  PrnsProvider,
  usePrns,
} from "personal-rns/solid";
import "./styles.css";

const AUTO_WIFI_POLL_INTERVAL_MS = 250;
const SNAPSHOT_POLL_INTERVAL_MS = 1_000;
const MAX_ACTIVITY_ENTRIES = 40;
const PERSISTENCE_ROOT = "prns.solid-auto-wifi";

type AutoWifiController = ReturnType<
  Prns["interfaces"]["autoWifi"]["start"]
>;

type AutoWifiRunState =
  | Tagged<"Ready">
  | Tagged<
      "Running",
      {
        readonly controller: AutoWifiController;
        readonly status: AutoWifiControllerStatus;
      }
    >
  | Tagged<
      "Closing",
      {
        readonly controller: AutoWifiController;
        readonly status: AutoWifiControllerStatus;
      }
    >
  | Tagged<"Closed">
  | Tagged<"Failed", { readonly detail: string }>;

type StatusPresentation = {
  readonly title: string;
  readonly detail: string;
  readonly tone: "waiting" | "working" | "active" | "failed" | "closed";
};

type ActivityEntry = {
  readonly occurredAt: number;
  readonly title: string;
  readonly detail: string;
};

function AutoWifiPanel() {
  const prns = usePrns();
  const [runState, setRunState] = createSignal<AutoWifiRunState>(Tag("Ready"));
  const [snapshot, setSnapshot] = createSignal<PrnsSnapshot>();
  const [activity, setActivity] = createSignal<readonly ActivityEntry[]>([]);
  let snapshotPending = false;
  let lastStatusSignature = "";

  const observedStatus = createMemo<AutoWifiControllerStatus | undefined>(() =>
    match_into<AutoWifiControllerStatus | undefined>().from(runState(), {
      Ready: () => undefined,
      Running: ({ status }) => status,
      Closing: ({ status }) => status,
      Closed: () => Tag("Closed"),
      Failed: () => undefined,
    })
  );
  const presentation = createMemo<StatusPresentation>(() => {
    const state = runState();
    if (state.tag === "Ready") {
      return {
        title: "Ready",
        detail: "The Prns node is ready to probe nearby browser gateways.",
        tone: "waiting",
      };
    }
    if (state.tag === "Failed") {
      return {
        title: "Controller failure",
        detail: state.data.detail,
        tone: "failed",
      };
    }
    const status = observedStatus();
    return status === undefined
      ? {
          title: "No status",
          detail: "The controller has not published a status.",
          tone: "waiting",
        }
      : presentAutoWifiStatus(status);
  });
  const gateways = createMemo<readonly AutoWifiGatewayStatus[]>(() => {
    const status = observedStatus();
    return status?.tag === "Active" ? status.data.gateways : [];
  });

  function record(title: string, detail: string): void {
    setActivity((current) => [
      { occurredAt: Date.now(), title, detail },
      ...current,
    ].slice(0, MAX_ACTIVITY_ENTRIES));
  }

  function recordStatus(status: AutoWifiControllerStatus): void {
    const signature = autoWifiStatusSignature(status);
    if (signature === lastStatusSignature) {
      return;
    }
    lastStatusSignature = signature;
    const current = presentAutoWifiStatus(status);
    record(current.title, current.detail);
  }

  function startAutoWifi(): void {
    const state = runState();
    if (state.tag === "Running" || state.tag === "Closing") {
      return;
    }
    const controller = prns.interfaces.autoWifi.start();
    const status = controller.status;
    lastStatusSignature = "";
    setRunState(Tag("Running", { controller, status }));
    record("Discovery started", "Probing localhost, prns.local, and local gateway catalogs.");
    recordStatus(status);
  }

  async function closeAutoWifi(): Promise<void> {
    const state = runState();
    if (state.tag !== "Running") {
      return;
    }
    setRunState(Tag("Closing", state.data));
    try {
      const outcome = await state.data.controller.close();
      match(outcome, {
        Closed: () => {
          setRunState(Tag("Closed"));
          record("Transport closed", "The Auto Wi-Fi controller released its gateway sessions.");
        },
        RuntimeRejected: ({ operation, detail }) => {
          const failure = `${operation}: ${detail}`;
          setRunState(Tag("Failed", { detail: failure }));
          record("Close rejected", failure);
        },
      });
    } catch (error: unknown) {
      const detail = describeUnknown(error);
      setRunState(Tag("Failed", { detail }));
      record("Close failed", detail);
    }
  }

  function refreshControllerStatus(): void {
    match(runState(), {
      Ready: () => undefined,
      Running: ({ controller }) => {
        const status = controller.status;
        setRunState(Tag("Running", { controller, status }));
        recordStatus(status);
      },
      Closing: ({ controller }) => {
        const status = controller.status;
        setRunState(Tag("Closing", { controller, status }));
        recordStatus(status);
      },
      Closed: () => undefined,
      Failed: () => undefined,
    });
  }

  async function refreshSnapshot(): Promise<void> {
    if (snapshotPending) {
      return;
    }
    snapshotPending = true;
    try {
      const outcome = await prns.snapshot();
      match(outcome, {
        Captured: (next) => setSnapshot(next),
        RuntimeRejected: ({ operation, detail }) => {
          record("Snapshot rejected", `${operation}: ${detail}`);
        },
      });
    } catch (error: unknown) {
      record("Snapshot failed", describeUnknown(error));
    } finally {
      snapshotPending = false;
    }
  }

  onMount(() => {
    startAutoWifi();
    void refreshSnapshot();
    const controllerTimer = globalThis.setInterval(
      refreshControllerStatus,
      AUTO_WIFI_POLL_INTERVAL_MS,
    );
    const snapshotTimer = globalThis.setInterval(
      () => void refreshSnapshot(),
      SNAPSHOT_POLL_INTERVAL_MS,
    );
    onCleanup(() => {
      globalThis.clearInterval(controllerTimer);
      globalThis.clearInterval(snapshotTimer);
      const state = runState();
      if (state.tag === "Running" || state.tag === "Closing") {
        void state.data.controller.close();
      }
    });
  });

  return (
    <main>
      <header class="hero">
        <div>
          <p class="eyebrow">personal-rns × SolidJS</p>
          <h1>Auto Wi-Fi, live and off-main-thread.</h1>
          <p class="lede">
            A real Prns browser node is probing your nearby native rendezvous
            transport. Solid redraws only the values whose signals change.
          </p>
        </div>
        <div class={`status-orb ${presentation().tone}`} aria-hidden="true">
          <span />
        </div>
      </header>

      <section class="status-card" data-status={presentation().tone}>
        <div>
          <p class="section-label">Auto Wi-Fi controller</p>
          <h2>{presentation().title}</h2>
          <p>{presentation().detail}</p>
        </div>
        <div class="actions">
          <button
            class="primary"
            disabled={runState().tag === "Running" || runState().tag === "Closing"}
            onClick={startAutoWifi}
          >
            Start Auto Wi-Fi
          </button>
          <button
            disabled={runState().tag !== "Running"}
            onClick={() => void closeAutoWifi()}
          >
            Close
          </button>
        </div>
      </section>

      <section class="metrics" aria-label="Prns node metrics">
        <Metric label="Execution" value={prns.execution} />
        <Metric label="Backend" value={prns.backendInfo.backend} />
        <Metric label="Interfaces" value={snapshot()?.interfaces.length ?? 0} />
        <Metric label="Routes" value={snapshot()?.routes ?? 0} />
        <Metric label="Active links" value={snapshot()?.activeLinkCount ?? 0} />
      </section>

      <div class="columns">
        <section class="panel">
          <div class="panel-heading">
            <div>
              <p class="section-label">Transport proof</p>
              <h2>Active gateways</h2>
            </div>
            <span class="count">{gateways().length}</span>
          </div>
          <Show
            when={gateways().length > 0}
            fallback={
              <p class="empty">
                Waiting for `localhost:42721` or `prns.local:42721` to answer.
              </p>
            }
          >
            <div class="gateway-list">
              <For each={gateways()}>
                {(gateway) => (
                  <article class="gateway">
                    <div class="gateway-title">
                      <strong>{gateway.localhost ? "Loopback gateway" : "LAN gateway"}</strong>
                      <span>active</span>
                    </div>
                    <code>{gateway.url}</code>
                    <dl>
                      <dt>Rendezvous</dt>
                      <dd>{gateway.id}</dd>
                      <dt>Interface</dt>
                      <dd>{hex(gateway.interfaceId)}</dd>
                    </dl>
                  </article>
                )}
              </For>
            </div>
          </Show>

          <div class="engine-interfaces">
            <h3>Engine interfaces</h3>
            <Show
              when={(snapshot()?.interfaces.length ?? 0) > 0}
              fallback={<p class="empty">No engine interfaces captured yet.</p>}
            >
              <For each={snapshot()?.interfaces ?? []}>
                {(interfaceSnapshot) => (
                  <div class="interface-row">
                    <span>{interfaceSnapshot.kind}</span>
                    <code>{hex(interfaceSnapshot.id)}</code>
                    <span>{interfaceSnapshot.routes} routes</span>
                  </div>
                )}
              </For>
            </Show>
          </div>
        </section>

        <section class="panel">
          <div class="panel-heading">
            <div>
              <p class="section-label">Reactive history</p>
              <h2>Controller activity</h2>
            </div>
            <span class="count">{activity().length}</span>
          </div>
          <Show
            when={activity().length > 0}
            fallback={<p class="empty">No status transitions yet.</p>}
          >
            <ol class="activity">
              <For each={activity()}>
                {(entry) => (
                  <li>
                    <time>{new Date(entry.occurredAt).toLocaleTimeString()}</time>
                    <div>
                      <strong>{entry.title}</strong>
                      <p>{entry.detail}</p>
                    </div>
                  </li>
                )}
              </For>
            </ol>
          </Show>
        </section>
      </div>

      <section class="code-card">
        <p class="section-label">The Solid-facing seam</p>
        <pre><code>{`const prns = usePrns()
const controller = prns.interfaces.autoWifi.start()

setStatus(controller.status)
const outcome = await controller.close()`}</code></pre>
      </section>
    </main>
  );
}

function Metric(props: { readonly label: string; readonly value: string | number }) {
  return (
    <article>
      <span>{props.label}</span>
      <strong>{props.value}</strong>
    </article>
  );
}

function presentAutoWifiStatus(status: AutoWifiControllerStatus): StatusPresentation {
  return match_into<StatusPresentation>().from(status, {
    Starting: () => ({
      title: "Transport starting",
      detail: "The Worker is preparing Auto Wi-Fi discovery.",
      tone: "working",
    }),
    Discovering: ({ attempt }) => ({
      title: `Discovery attempt ${attempt}`,
      detail: "Probing direct loopback, local-name, and gateway-catalog candidates.",
      tone: "working",
    }),
    Active: ({ gateways }) => ({
      title: `${gateways.length} gateway${gateways.length === 1 ? "" : "s"} active`,
      detail: gateways.map(({ url }) => url).join(" · "),
      tone: "active",
    }),
    Unavailable: (failure) => ({
      title: "Auto Wi-Fi unavailable",
      detail: describeAutoWifiFailure(failure),
      tone: "failed",
    }),
    Closed: () => ({
      title: "Transport closed",
      detail: "Start Auto Wi-Fi to begin another discovery run.",
      tone: "closed",
    }),
  });
}

function describeAutoWifiFailure(failure: AutoWifiFailure): string {
  return match_into<string>().from(failure, {
    HostApiUnavailable: ({ api }) => `${api} is unavailable in this browser.`,
    PermissionDenied: ({ stage, detail }) => `${stage}: ${detail}`,
    AlreadyActive: ({ target }) => `Auto Wi-Fi is already active for ${target}.`,
    SelectionIdentityUnavailable: ({ detail }) => detail,
    DiscoveryFailed: ({ detail }) => detail,
    RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
  });
}

function autoWifiStatusSignature(status: AutoWifiControllerStatus): string {
  return match_into<string>().from(status, {
    Starting: () => "Starting",
    Discovering: ({ attempt }) => `Discovering:${attempt}`,
    Active: ({ gateways }) =>
      `Active:${gateways.map(({ id, url }) => `${id}:${url}`).join("|")}`,
    Unavailable: (failure) => `Unavailable:${describeAutoWifiFailure(failure)}`,
    Closed: () => "Closed",
  });
}

function hex(bytes: Uint8Array): string {
  let value = "";
  for (const byte of bytes) {
    value += byte.toString(16).padStart(2, "0");
  }
  return value;
}

function describeUnknown(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function StartupFailure(props: { readonly outcome: Exclude<PrnsCreateOutcome, { readonly tag: "Ready" }> }) {
  return (
    <main class="booting failure-page">
      <p class="eyebrow">personal-rns × SolidJS</p>
      <h1>Prns did not start.</h1>
      <p>{props.outcome.tag}</p>
      <pre><code>{JSON.stringify(props.outcome.data, jsonReplacer, 2)}</code></pre>
    </main>
  );
}

function StartupCrash(props: { readonly detail: string }) {
  return (
    <main class="booting failure-page">
      <p class="eyebrow">personal-rns × SolidJS</p>
      <h1>Prns startup crashed.</h1>
      <pre><code>{props.detail}</code></pre>
    </main>
  );
}

function jsonReplacer(_key: string, value: unknown): unknown {
  return typeof value === "bigint" ? value.toString() : value;
}

async function start(): Promise<void> {
  const root = document.getElementById("root");
  if (root === null) {
    throw new Error("Solid Auto Wi-Fi root element is missing");
  }
  const wasmModuleUrl = new URL("/prns_wasm.js", globalThis.location.href);
  const created = await Prns.create({
    ...persistentBrowser(PERSISTENCE_ROOT),
    execution: "DedicatedWorker",
    wasmModuleUrl,
    resourceCompressionModuleUrl: wasmModuleUrl,
  });
  if (created.tag !== "Ready") {
    root.replaceChildren();
    render(() => <StartupFailure outcome={created} />, root);
    return;
  }
  const prns = created.data;
  root.replaceChildren();
  const dispose = render(
    () => (
      <PrnsProvider prns={prns}>
        <AutoWifiPanel />
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
    render(() => <StartupCrash detail={describeUnknown(error)} />, root);
  }
});
