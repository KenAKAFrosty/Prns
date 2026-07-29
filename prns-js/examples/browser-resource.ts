import {
  Prns,
  Tag,
} from "../src/browser/index.js";
import type {
  LinkId,
  PrnsWasmModule,
} from "../src/browser/index.js";

export async function sendFile(
  wasm: PrnsWasmModule,
  linkId: LinkId,
  file: Blob,
): Promise<void> {
  const created = await Prns.create({ wasm });
  if (created.tag !== "Ready") {
    throw new Error(`browser node creation failed: ${created.tag}`);
  }
  const node = created.data;
  const sent = await node.sendResourceBlob(linkId, file, {
    compression: Tag("Auto"),
  });
  if (sent.tag === "Failed") {
    const commandFailure = sent.data;
    throw new Error(commandFailure.tag);
  }
}
