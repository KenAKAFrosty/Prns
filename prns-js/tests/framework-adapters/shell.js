import { noSerialize, render } from "@builder.io/qwik";
import { jsx } from "@builder.io/qwik/jsx-runtime";
import { Tag } from "personal-rns/browser";
import {
  PRNS_BRIDGE_CHANGE_EVENT,
  definePrnsBridgeElement,
} from "personal-rns/web-component";
import { QwikAdapterTest } from "adapter-qwik-consumer";
import { mountCommonAdapters } from "./common.js";
import { FakePrns } from "./fake_prns.js";
import { mountSvelteAdapter } from "./svelte.js";

const expectedFrameworks = Object.freeze([
  "qwik",
  "react",
  "solid",
  "svelte",
  "vue",
  "web-component",
]);

run().catch(reportFailure);

async function run() {
  const prns = new FakePrns();
  const releaseCommon = mountCommonAdapters(prns);
  const releaseSvelte = mountSvelteAdapter(prns);
  const releaseQwik = await render(
    requireElement("qwik-target"),
    jsx(QwikAdapterTest, { prns: noSerialize(prns) }),
  );
  const bridge = mountWebComponent(prns);

  await waitUntil(() =>
    readFrameworks().length === expectedFrameworks.length &&
    readStates().every((state) => state === "Starting") &&
    prns.activeSubscriptions === 10
  );

  prns.publishLifecycle(Tag("Running"));
  await waitUntil(() => readStates().every((state) => state === "Running"));

  const frameworks = readFrameworks();
  const states = readStates();
  const peakSubscriptions = prns.activeSubscriptions;

  releaseCommon();
  await releaseSvelte();
  releaseQwik.cleanup();
  bridge.remove();

  await waitUntil(() => prns.activeSubscriptions === 0);
  const deliveriesBeforeCleanup = prns.deliveries;
  prns.publishLifecycle(Tag("Stopping"));
  await new Promise((resolveWait) => setTimeout(resolveWait, 20));

  await reportResult({
    ready: true,
    frameworks,
    states,
    peakSubscriptions,
    remainingSubscriptions: prns.activeSubscriptions,
    deliveriesBeforeCleanup,
    deliveriesAfterCleanup: prns.deliveries,
  });
}

function mountWebComponent(prns) {
  definePrnsBridgeElement();
  const bridge = document.createElement("prns-bridge");
  const output = document.createElement("output");
  output.dataset.framework = "web-component";
  bridge.addEventListener(PRNS_BRIDGE_CHANGE_EVENT, (event) => {
    const state = event.detail.lifecycle.tag;
    output.dataset.state = state;
    output.textContent = state;
  });
  bridge.append(output);
  requireElement("web-component-target").append(bridge);
  bridge.configure({ prns, diagnosticMaximumEvents: 16 });
  return bridge;
}

function readFrameworks() {
  return [...document.querySelectorAll("[data-framework]")]
    .map((element) => element.dataset.framework)
    .sort();
}

function readStates() {
  return [...document.querySelectorAll("[data-framework]")]
    .map((element) => element.dataset.state);
}

function requireElement(id) {
  const element = document.getElementById(id);
  if (element === null) {
    throw new Error(`framework adapter test target ${id} is missing`);
  }
  return element;
}

async function waitUntil(predicate) {
  const deadline = performance.now() + 10_000;
  while (!predicate()) {
    if (performance.now() >= deadline) {
      throw new Error(`framework adapter lifecycle timed out: ${JSON.stringify({
        frameworks: readFrameworks(),
        states: readStates(),
      })}`);
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 5));
  }
}

async function reportFailure(error) {
  await reportResult({
    ready: false,
    error: error instanceof Error ? error.stack ?? error.message : String(error),
  });
}

function reportResult(result) {
  return fetch("/framework-adapter-result", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(result),
  });
}
