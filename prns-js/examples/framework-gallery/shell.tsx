import {
  noSerialize,
  render,
} from "@builder.io/qwik";
import { jsx } from "@builder.io/qwik/jsx-runtime";
import { Prns } from "personal-rns/browser";
import {
  PRNS_BRIDGE_CHANGE_EVENT,
  definePrnsBridgeElement,
} from "personal-rns/web-component";
import type {
  PrnsBridgeElement,
  PrnsBridgeState,
} from "personal-rns/web-component";
import { QwikGallery } from "./dist-qwik/QwikPanel.qwik.mjs";
import { GalleryJourney } from "./journey.js";
import "./runtime.js";

export async function start(): Promise<void> {
  const created = await Prns.create({
    wasmModuleUrl: new URL("/prns_wasm.js", globalThis.location.href),
  });
  if (created.tag !== "Ready") {
    throw new Error(`Prns startup failed: ${created.tag}`);
  }
  const prns = created.data;
  globalThis.window.prnsFrameworkGallery = { prns };
  await render(
    requireElement("qwik-panel"),
    jsx(QwikGallery, {
      prns: noSerialize(prns),
    }),
  );
  definePrnsBridgeElement();
  const bridge = requireElement("prns-bridge") as PrnsBridgeElement;
  bridge.configure({ prns, diagnosticMaximumEvents: 32 });
  bridge.addEventListener(PRNS_BRIDGE_CHANGE_EVENT, (event) => {
    const state = (event as CustomEvent<PrnsBridgeState>).detail;
    setText("bridge-lifecycle", state.lifecycle.tag);
    setText("bridge-interfaces", state.interfaces.length);
    setText("bridge-routes", state.routes.length);
    setText("bridge-links", state.links.length);
    setText("bridge-diagnostics", state.diagnostics.length);
    setProjectionAttributes(
      requireElement("web-component-panel"),
      state.interfaces.length,
      state.routes.length,
      state.links.length,
      state.diagnostics.length,
    );
  });
  setText("gallery-status", `Ready · ${prns.execution} · ${prns.backendInfo.backend}`);
  const journey = new GalleryJourney(prns);
  await Promise.all([
    journey.loadSession(),
    loadPanel("common.mjs"),
    loadPanel("svelte.mjs"),
  ]);
  await waitForPanels();
  document.documentElement.dataset.gallery = "Ready";
}

function loadPanel(file: string): Promise<void> {
  const script = document.createElement("script");
  script.type = "module";
  script.src = new URL(file, document.baseURI).href;
  return new Promise((resolve, reject) => {
    script.addEventListener("load", () => resolve(), { once: true });
    script.addEventListener(
      "error",
      () => reject(new Error(`gallery module ${file} failed to load`)),
      { once: true },
    );
    document.head.append(script);
  });
}

async function waitForPanels(): Promise<void> {
  const deadline = performance.now() + 5_000;
  while (document.querySelectorAll("[data-framework]").length < 6) {
    if (performance.now() >= deadline) {
      const mounted = [...document.querySelectorAll<HTMLElement>("[data-framework]")]
        .map((element) => element.dataset.framework)
        .join(",");
      throw new Error(`framework gallery panels did not mount in time: ${mounted}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
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

function setProjectionAttributes(
  element: HTMLElement,
  interfaces: number,
  routes: number,
  links: number,
  diagnostics: number,
): void {
  element.dataset.interfaces = String(interfaces);
  element.dataset.routes = String(routes);
  element.dataset.links = String(links);
  element.dataset.diagnostics = String(diagnostics);
}
