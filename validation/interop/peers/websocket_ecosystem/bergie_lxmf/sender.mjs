import path from "node:path";
import { writeFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";


const repository = process.argv[2];
const target = process.argv[3];
const targetHashHex = process.argv[4];
const readySignal = process.argv[5];
const coreUrl = pathToFileURL(path.join(repository, "packages/core/src/index.js"));
const lxmfUrl = pathToFileURL(path.join(repository, "packages/core/src/lxmf/index.js"));
const websocketUrl = pathToFileURL(
  path.join(repository, "packages/core/src/interfaces/websocket.js"),
);
const {
  Destination,
  fromHex,
  Identity,
  MemoryStorageAdapter,
  Reticulum,
  toHex,
} = await import(coreUrl);
const { LXMessage, LXMRouter } = await import(lxmfUrl);
const { WebSocketClientInterface } = await import(websocketUrl);


const targetHash = fromHex(targetHashHex);
const rns = new Reticulum({ storageAdapter: new MemoryStorageAdapter() });
const websocket = new WebSocketClientInterface({
  url: target,
  framing: "raw",
  autoReconnect: false,
  name: "Bergie raw sender",
});
await websocket.connect();
rns.addInterface(websocket, true);

const identity = await Identity.generate();
const lxmf = new LXMRouter(identity, rns);
await lxmf.init();

let finished = false;
let failureTimer;


const stop = async (code) => {
  if (finished) return;
  finished = true;
  clearTimeout(failureTimer);
  await rns.stop();
  process.exit(code);
};


const waitForIdentity = async () => {
  const deadline = Date.now() + 130000;
  while (Date.now() < deadline) {
    const peerIdentity = await Destination.recall(targetHash);
    if (peerIdentity) return peerIdentity;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`did not learn ${targetHashHex} from an announce`);
};


lxmf.addEventListener("message", async (event) => {
  const message = event.detail.message;
  console.log(`SENDER_RECEIVED ${message.content}`);
  await stop(message.content === "Echo: Hello through real prnsd" ? 0 : 1);
});


console.log(`SENDER_DESTINATION ${toHex(lxmf.deliveryDest.destinationHash)}`);
await lxmf.announce("Bergie raw sender");
console.log("SENDER_ANNOUNCED");
await writeFile(readySignal, "ready\n");
await waitForIdentity();
console.log(`SENDER_LEARNED ${targetHashHex}`);

const message = new LXMessage({
  sourceHash: lxmf.deliveryDest.destinationHash,
  destinationHash: targetHash,
  title: "Prns WebSocket auto",
  content: "Hello through real prnsd",
});
await lxmf.send(message, identity);
console.log("SENDER_SENT Hello through real prnsd");
failureTimer = setTimeout(async () => {
  console.error("SENDER_TIMEOUT");
  await stop(1);
}, 15000);
