import {
  Tag,
  destinationHash,
  match,
} from "personal-rns/browser";
import type {
  DestinationHash,
  Prns,
  Tag as Tagged,
} from "personal-rns/browser";

type GallerySession = {
  readonly destination: DestinationHash;
  readonly destinationHex: string;
  readonly webSocketUrl: string;
};

type GalleryJourneyState =
  | Tagged<"LoadingSession">
  | Tagged<"Ready", GallerySession>
  | Tagged<"Connecting", GallerySession>
  | Tagged<"Connected", GallerySession>
  | Tagged<"Discovering", GallerySession>
  | Tagged<"Routed", GallerySession>
  | Tagged<"EstablishingLink", GallerySession>
  | Tagged<"Linked", GallerySession>
  | Tagged<"SessionFailed", { readonly detail: string }>
  | Tagged<"ConnectFailed", GallerySession & { readonly detail: string }>
  | Tagged<"PathFailed", GallerySession & { readonly detail: string }>
  | Tagged<"LinkFailed", GallerySession & { readonly detail: string }>
  | Tagged<"Stopped">;

export class GalleryJourney {
  readonly #prns: Prns;
  #state: GalleryJourneyState = Tag("LoadingSession");

  constructor(prns: Prns) {
    this.#prns = prns;
    this.#bindControls();
    this.#render();
  }

  async loadSession(): Promise<void> {
    try {
      const response = await fetch("/api/gallery-session");
      if (!response.ok) {
        throw new Error(`session request returned HTTP ${response.status}`);
      }
      const session = gallerySession(await response.json());
      const input = requireElement("websocket-url") as HTMLInputElement;
      input.value = session.webSocketUrl;
      this.#replace(Tag("Ready", session));
    } catch (error) {
      this.#replace(Tag("SessionFailed", { detail: detail(error) }));
    }
  }

  async #connect(): Promise<void> {
    const session = match(this.#state, {
      Ready: (value) => value,
      ConnectFailed: ({ detail: _detail, ...value }) => value,
      LoadingSession: () => undefined,
      Connecting: () => undefined,
      Connected: () => undefined,
      Discovering: () => undefined,
      Routed: () => undefined,
      EstablishingLink: () => undefined,
      Linked: () => undefined,
      SessionFailed: () => undefined,
      PathFailed: () => undefined,
      LinkFailed: () => undefined,
      Stopped: () => undefined,
    });
    if (session === undefined) {
      return;
    }
    const input = requireElement("websocket-url") as HTMLInputElement;
    this.#replace(Tag("Connecting", session));
    try {
      const outcome = await this.#prns.interfaces.webSocket.connect(input.value);
      showOutcome(outcome);
      this.#replace(outcome.tag === "Connected"
        ? Tag("Connected", session)
        : Tag("ConnectFailed", {
            ...session,
            detail: outcomeText(outcome),
          }));
    } catch (error) {
      showError(error);
      this.#replace(Tag("ConnectFailed", {
        ...session,
        detail: detail(error),
      }));
    }
  }

  async #discover(): Promise<void> {
    const session = match(this.#state, {
      Connected: (value) => value,
      PathFailed: ({ detail: _detail, ...value }) => value,
      LoadingSession: () => undefined,
      Ready: () => undefined,
      Connecting: () => undefined,
      Discovering: () => undefined,
      Routed: () => undefined,
      EstablishingLink: () => undefined,
      Linked: () => undefined,
      SessionFailed: () => undefined,
      ConnectFailed: () => undefined,
      LinkFailed: () => undefined,
      Stopped: () => undefined,
    });
    if (session === undefined) {
      return;
    }
    this.#replace(Tag("Discovering", session));
    try {
      const outcome = await this.#prns.requestPath(session.destination);
      showOutcome(outcome);
      this.#replace(
        outcome.tag === "Succeeded" && outcome.data.tag === "PathDiscovered"
          ? Tag("Routed", session)
          : Tag("PathFailed", {
              ...session,
              detail: outcomeText(outcome),
            }),
      );
    } catch (error) {
      showError(error);
      this.#replace(Tag("PathFailed", {
        ...session,
        detail: detail(error),
      }));
    }
  }

  async #establishLink(): Promise<void> {
    const session = match(this.#state, {
      Routed: (value) => value,
      LinkFailed: ({ detail: _detail, ...value }) => value,
      LoadingSession: () => undefined,
      Ready: () => undefined,
      Connecting: () => undefined,
      Connected: () => undefined,
      Discovering: () => undefined,
      EstablishingLink: () => undefined,
      Linked: () => undefined,
      SessionFailed: () => undefined,
      ConnectFailed: () => undefined,
      PathFailed: () => undefined,
      Stopped: () => undefined,
    });
    if (session === undefined) {
      return;
    }
    this.#replace(Tag("EstablishingLink", session));
    try {
      const outcome = await this.#prns.establishLink(session.destination);
      showOutcome(outcome);
      this.#replace(
        outcome.tag === "Succeeded" && outcome.data.tag === "LinkEstablished"
          ? Tag("Linked", session)
          : Tag("LinkFailed", {
              ...session,
              detail: outcomeText(outcome),
            }),
      );
    } catch (error) {
      showError(error);
      this.#replace(Tag("LinkFailed", {
        ...session,
        detail: detail(error),
      }));
    }
  }

  async #run(): Promise<void> {
    if (this.#state.tag === "Ready" || this.#state.tag === "ConnectFailed") {
      await this.#connect();
    }
    if (this.#state.tag === "Connected" || this.#state.tag === "PathFailed") {
      await this.#discover();
    }
    if (this.#state.tag === "Routed" || this.#state.tag === "LinkFailed") {
      await this.#establishLink();
    }
  }

  #bindControls(): void {
    requireElement("connect-websocket").addEventListener("click", () => {
      void this.#connect();
    });
    requireElement("request-path").addEventListener("click", () => {
      void this.#discover();
    });
    requireElement("establish-link").addEventListener("click", () => {
      void this.#establishLink();
    });
    requireElement("run-journey").addEventListener("click", () => {
      void this.#run();
    });
    requireElement("connect-bluetooth").addEventListener("click", () => {
      void this.#prns.interfaces.bluetooth.connect().then(showOutcome, showError);
    });
    requireElement("stop-prns").addEventListener("click", () => {
      void this.#prns.stop().then((outcome) => {
        showOutcome(outcome);
        this.#replace(Tag("Stopped"));
      }, showError);
    });
  }

  #replace(state: GalleryJourneyState): void {
    this.#state = state;
    this.#render();
  }

  #render(): void {
    document.documentElement.dataset.journey = this.#state.tag;
    setText("journey-state", journeyText(this.#state));
    const session = journeySession(this.#state);
    setText("companion-destination", session?.destinationHex ?? "Unavailable");
    setDisabled("connect-websocket", !(
      this.#state.tag === "Ready" || this.#state.tag === "ConnectFailed"
    ));
    setDisabled("request-path", !(
      this.#state.tag === "Connected" || this.#state.tag === "PathFailed"
    ));
    setDisabled("establish-link", !(
      this.#state.tag === "Routed" || this.#state.tag === "LinkFailed"
    ));
    setDisabled("run-journey", ![
      "Ready",
      "ConnectFailed",
      "Connected",
      "PathFailed",
      "Routed",
      "LinkFailed",
    ].includes(this.#state.tag));
    setDisabled("stop-prns", this.#state.tag === "Stopped");
  }
}

function gallerySession(value: unknown): GallerySession {
  if (typeof value !== "object" || value === null) {
    throw new Error("gallery session must be an object");
  }
  const candidate = value as Record<string, unknown>;
  if (
    typeof candidate.webSocketUrl !== "string" ||
    typeof candidate.destinationHex !== "string"
  ) {
    throw new Error("gallery session is missing its WebSocket URL or destination");
  }
  return {
    destination: destinationHash(hexadecimalBytes(candidate.destinationHex)),
    destinationHex: candidate.destinationHex,
    webSocketUrl: candidate.webSocketUrl,
  };
}

function hexadecimalBytes(value: string): Uint8Array {
  if (!/^[0-9a-f]{32}$/iu.test(value)) {
    throw new Error("gallery destination must contain 32 hexadecimal digits");
  }
  return Uint8Array.from(
    { length: value.length / 2 },
    (_, index) => Number.parseInt(value.slice(index * 2, index * 2 + 2), 16),
  );
}

function journeyText(state: GalleryJourneyState): string {
  return match(state, {
    LoadingSession: () => "Loading companion session",
    Ready: () => "Ready to connect",
    Connecting: () => "Connecting WebSocket",
    Connected: () => "WebSocket connected",
    Discovering: () => "Requesting the companion path",
    Routed: () => "Companion route discovered",
    EstablishingLink: () => "Establishing an encrypted link",
    Linked: () => "Full two-engine journey complete",
    SessionFailed: ({ detail: value }) => `Session failed · ${value}`,
    ConnectFailed: ({ detail: value }) => `Connection failed · ${value}`,
    PathFailed: ({ detail: value }) => `Path discovery failed · ${value}`,
    LinkFailed: ({ detail: value }) => `Link establishment failed · ${value}`,
    Stopped: () => "Prns stopped",
  });
}

function journeySession(state: GalleryJourneyState): GallerySession | undefined {
  return match(state, {
    LoadingSession: () => undefined,
    Ready: (value) => value,
    Connecting: (value) => value,
    Connected: (value) => value,
    Discovering: (value) => value,
    Routed: (value) => value,
    EstablishingLink: (value) => value,
    Linked: (value) => value,
    SessionFailed: () => undefined,
    ConnectFailed: ({ detail: _detail, ...value }) => value,
    PathFailed: ({ detail: _detail, ...value }) => value,
    LinkFailed: ({ detail: _detail, ...value }) => value,
    Stopped: () => undefined,
  });
}

function showOutcome(value: unknown): void {
  setText("command-outcome", outcomeText(value));
}

function showError(error: unknown): void {
  setText("command-outcome", detail(error));
}

function outcomeText(value: unknown): string {
  return JSON.stringify(value, (_, item) =>
    typeof item === "bigint" ? item.toString() : item
  );
}

function detail(value: unknown): string {
  return value instanceof Error ? `${value.name}: ${value.message}` : String(value);
}

function requireElement(id: string): HTMLElement {
  const element = document.getElementById(id);
  if (element === null) {
    throw new Error(`gallery element ${id} is missing`);
  }
  return element;
}

function setText(id: string, value: string | number): void {
  requireElement(id).textContent = String(value);
}

function setDisabled(id: string, disabled: boolean): void {
  (requireElement(id) as HTMLButtonElement).disabled = disabled;
}
