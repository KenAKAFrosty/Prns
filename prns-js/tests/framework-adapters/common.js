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

export function mountCommonAdapters(prns) {
  const reactRoot = createRoot(requireElement("react-target"));
  reactRoot.render(createElement(
    ReactPrnsProvider,
    { prns },
    createElement(ReactConsumer),
  ));

  const disposeSolid = renderSolid(
    () => createComponent(SolidPrnsProvider, {
      prns,
      get children() {
        return createComponent(SolidConsumer, {});
      },
    }),
    requireElement("solid-target"),
  );

  const vueApp = createApp({
    setup() {
      providePrns(prns);
      return () => h(VueConsumer);
    },
  });
  vueApp.mount(requireElement("vue-target"));

  return () => {
    reactRoot.unmount();
    disposeSolid();
    vueApp.unmount();
  };
}

function ReactConsumer() {
  const lifecycle = useReactPrnsProjection(prnsView("Lifecycle"));
  return createElement("output", {
    "data-framework": "react",
    "data-state": lifecycle.tag,
  }, lifecycle.tag);
}

function SolidConsumer() {
  const lifecycle = createPrnsProjection(prnsView("Lifecycle"));
  const output = document.createElement("output");
  output.dataset.framework = "solid";
  createEffect(() => {
    const state = lifecycle().tag;
    output.dataset.state = state;
    output.textContent = state;
  });
  return output;
}

const VueConsumer = {
  setup() {
    const lifecycle = useVuePrnsProjection(prnsView("Lifecycle"));
    return () => h("output", {
      "data-framework": "vue",
      "data-state": lifecycle.value.tag,
    }, lifecycle.value.tag);
  },
};

function requireElement(id) {
  const element = document.getElementById(id);
  if (element === null) {
    throw new Error(`framework adapter test target ${id} is missing`);
  }
  return element;
}
