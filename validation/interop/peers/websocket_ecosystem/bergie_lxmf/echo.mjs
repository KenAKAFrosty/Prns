import path from "node:path";
import { access } from "node:fs/promises";
import { pathToFileURL } from "node:url";


const repository = process.argv[2];
const target = process.argv[3];
const readySignal = process.argv[4];
const coreUrl = pathToFileURL(path.join(repository, "packages/core/src/index.js"));
const lxmfUrl = pathToFileURL(path.join(repository, "packages/core/src/lxmf/index.js"));
const websocketUrl = pathToFileURL(
  path.join(repository, "packages/core/src/interfaces/websocket.js"),
);
const { Identity, MemoryStorageAdapter, Reticulum, toHex } = await import(coreUrl);
const { LXMessage, LXMRouter } = await import(lxmfUrl);
const { WebSocketClientInterface } = await import(websocketUrl);


const rns = new Reticulum({ storageAdapter: new MemoryStorageAdapter() });
const websocket = new WebSocketClientInterface({
  url: target,
  framing: "kiss",
  autoReconnect: false,
  name: "Bergie KISS echo bot",
});
await websocket.connect();
rns.addInterface(websocket, true);

const identity = await Identity.generate();
const lxmf = new LXMRouter(identity, rns);
await lxmf.init();

let replying = false;
let finished = false;
let failureTimer;


const stop = async (code) => {
  if (finished) return;
  finished = true;
  clearTimeout(failureTimer);
  lxmf.stopAnnouncing();
  await rns.stop();
  process.exit(code);
};


const waitForReadySignal = async () => {
  while (true) {
    try {
      await access(readySignal);
      return;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  }
};


lxmf.addEventListener("message", async (event) => {
  if (replying) return;
  replying = true;
  const message = event.detail.message;
  const link = event.detail.link;
  console.log(`ECHO_RECEIVED ${message.content}`);
  const reply = new LXMessage({
    sourceHash: lxmf.deliveryDest.destinationHash,
    destinationHash: message.sourceHash,
    title: `Re: ${message.title}`,
    content: `Echo: ${message.content}`,
  });
  try {
    await lxmf.send(reply, identity, link);
    console.log(`ECHO_REPLIED ${reply.content}`);
    await new Promise((resolve) => setTimeout(resolve, 500));
    await stop(0);
  } catch (error) {
    console.error(error instanceof Error ? error.stack : error);
    await stop(1);
  }
});


console.log(`ECHO_DESTINATION ${toHex(lxmf.deliveryDest.destinationHash)}`);
console.log("ECHO_WAITING_FOR_SENDER");
await waitForReadySignal();
await lxmf.startAnnouncing("Bergie KISS echo bot", { intervalMs: 60_000 });
console.log("ECHO_ANNOUNCED");
failureTimer = setTimeout(async () => {
  console.error("ECHO_TIMEOUT");
  await stop(1);
}, 160000);
process.on("SIGTERM", () => {
  stop(0).catch(() => {});
});
