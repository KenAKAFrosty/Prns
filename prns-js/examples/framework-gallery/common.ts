import { createElement } from "react";
import { createRoot } from "react-dom/client";
import { createComponent, createEffect } from "solid-js";
import { render as renderSolid } from "solid-js/web";
import { createApp, h } from "vue";
import { prnsView } from "personal-rns/browser";
import {
  PrnsProvider as ReactPrnsProvider,
  usePrnsProjection as useReactPrnsProjection,
} from "personal-rns/react";
import {
  PrnsProvider as SolidPrnsProvider,
  createPrnsProjection,
} from "personal-rns/solid";
import {
  providePrns,
  usePrnsProjection as useVuePrnsProjection,
} from "personal-rns/vue";
import { galleryRuntime } from "./runtime.js";

const { prns } = galleryRuntime();
const labels = ["Lifecycle", "Interfaces", "Routes", "Links", "Diagnostics"];

function ReactPanel() {
  const lifecycle = useReactPrnsProjection(prnsView("Lifecycle"));
  const interfaces = useReactPrnsProjection(prnsView("Interfaces"));
  const routes = useReactPrnsProjection(prnsView("Routes"));
  const links = useReactPrnsProjection(prnsView("Links"));
  const diagnostics = useReactPrnsProjection(
    prnsView("Diagnostics", { maximumEvents: 32 }),
  );
  return panel("React", lifecycle.tag, interfaces.length, routes.length, links.length, diagnostics.length);
}

createRoot(requireElement("react-panel")).render(
  createElement(
    ReactPrnsProvider,
    { prns },
    createElement(ReactPanel),
  ),
);

function SolidPanel() {
  const lifecycle = createPrnsProjection(prnsView("Lifecycle"));
  const interfaces = createPrnsProjection(prnsView("Interfaces"));
  const routes = createPrnsProjection(prnsView("Routes"));
  const links = createPrnsProjection(prnsView("Links"));
  const diagnostics = createPrnsProjection(
    prnsView("Diagnostics", { maximumEvents: 32 }),
  );
  const article = document.createElement("article");
  article.className = "panel";
  article.dataset.framework = "solid";
  const heading = document.createElement("h2");
  heading.textContent = "Solid";
  const values = document.createElement("dl");
  article.append(heading, values);
  createRows(article, values, () => [
    lifecycle().tag,
    interfaces().length,
    routes().length,
    links().length,
    diagnostics().length,
  ]);
  return article;
}

renderSolid(
  () => createComponent(SolidPrnsProvider, {
    prns,
    get children() {
      return createComponent(SolidPanel, {});
    },
  }),
  requireElement("solid-panel"),
);

const VuePanel = {
  setup() {
    const lifecycle = useVuePrnsProjection(prnsView("Lifecycle"));
    const interfaces = useVuePrnsProjection(prnsView("Interfaces"));
    const routes = useVuePrnsProjection(prnsView("Routes"));
    const links = useVuePrnsProjection(prnsView("Links"));
    const diagnostics = useVuePrnsProjection(
      prnsView("Diagnostics", { maximumEvents: 32 }),
    );
    return () => h("article", {
      class: "panel",
      "data-diagnostics": diagnostics.value.length,
      "data-framework": "vue",
      "data-interfaces": interfaces.value.length,
      "data-links": links.value.length,
      "data-routes": routes.value.length,
    }, [
      h("h2", "Vue"),
      rows([
        lifecycle.value.tag,
        interfaces.value.length,
        routes.value.length,
        links.value.length,
        diagnostics.value.length,
      ]),
    ]);
  },
};

createApp({
  setup() {
    providePrns(prns);
    return () => h(VuePanel);
  },
}).mount(requireElement("vue-panel"));

function panel(
  name: string,
  lifecycle: string,
  interfaces: number,
  routes: number,
  links: number,
  diagnostics: number,
) {
  return createElement("article", {
    className: "panel",
    "data-diagnostics": diagnostics,
    "data-framework": name.toLowerCase(),
    "data-interfaces": interfaces,
    "data-links": links,
    "data-routes": routes,
  }, [
    createElement("h2", { key: "heading" }, name),
    createElement("dl", { key: "values" }, rowElements([
      lifecycle,
      interfaces,
      routes,
      links,
      diagnostics,
    ])),
  ]);
}

function rowElements(values: readonly (string | number)[]) {
  return labels.flatMap((label, index) => [
    createElement("dt", { key: `${label}-label` }, label),
    createElement("dd", { key: `${label}-value` }, values[index]),
  ]);
}

function rows(values: readonly (string | number)[]) {
  return h("dl", labels.flatMap((label, index) => [
    h("dt", { key: `${label}-label` }, label),
    h("dd", { key: `${label}-value` }, values[index]),
  ]));
}

function createRows(
  article: HTMLElement,
  target: HTMLElement,
  values: () => readonly (string | number)[],
): void {
  const entries = labels.map((label) => {
    const term = document.createElement("dt");
    term.textContent = label;
    const detail = document.createElement("dd");
    target.append(term, detail);
    return detail;
  });
  const render = () => {
    const current = values();
    article.dataset.interfaces = String(current[1]);
    article.dataset.routes = String(current[2]);
    article.dataset.links = String(current[3]);
    article.dataset.diagnostics = String(current[4]);
    for (let index = 0; index < entries.length; index += 1) {
      const detail = entries[index];
      if (detail !== undefined) {
        detail.textContent = String(current[index]);
      }
    }
  };
  createEffect(render);
}

function requireElement(id: string): HTMLElement {
  const element = document.getElementById(id);
  if (element === null) {
    throw new Error(`gallery element ${id} is missing`);
  }
  return element;
}
