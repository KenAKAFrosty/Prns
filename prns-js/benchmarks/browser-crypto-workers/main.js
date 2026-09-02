import { Tag, match } from "../../dist/casework.js";

const resultElement = document.querySelector("#result");
const coordinator = new Worker("./coordinator.js", {
  type: "module",
  name: "prns-crypto-benchmark-engine",
});
const probe = new Worker("./probe.js", {
  type: "module",
  name: "prns-crypto-benchmark-probe",
});
const probeChannel = new MessageChannel();

coordinator.addEventListener("message", ({ data }) => {
  match(data, {
    Completed: (result) => {
      resultElement.textContent = JSON.stringify(result, null, 2);
      void report(result);
    },
    Failed: ({ detail }) => {
      resultElement.textContent = detail;
      void report({ error: detail });
    },
  });
});
coordinator.addEventListener("error", (event) => {
  const detail = event.message || "crypto benchmark coordinator failed";
  resultElement.textContent = detail;
  void report({ error: detail });
});
probe.postMessage(
  Tag("Initialize", { port: probeChannel.port1 }),
  [probeChannel.port1],
);
coordinator.postMessage(
  Tag("Initialize", { probe: probeChannel.port2 }),
  [probeChannel.port2],
);

async function report(result) {
  await fetch("/browser-crypto-workers-result", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(result),
  });
}
