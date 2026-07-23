import type { PrnsNode } from '../../index.js';

export const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

export const bufEq = (a: Uint8Array | undefined, b: Uint8Array | undefined): boolean =>
  !!a && !!b && Buffer.compare(Buffer.from(a), Buffer.from(b)) === 0;

export async function waitFor(predicate: () => boolean, timeoutMs: number, label: string): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await sleep(50);
  }
  throw new Error(`timeout waiting for ${label}`);
}

export type AnyEvent = Record<string, any>;

export async function announceUntilHeard(
  from: PrnsNode,
  destination: Buffer,
  target: AnyEvent[],
  label: string
): Promise<void> {
  for (let i = 0; i < 24; i += 1) {
    await from.announce(destination);
    await sleep(500);
    if (target.some((e) => e.type === 'announce' && bufEq(e.destination, destination))) return;
  }
  throw new Error(`announce never heard: ${label}`);
}
