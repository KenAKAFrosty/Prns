import assert from "node:assert/strict";
import test from "node:test";
import { usePrns as useQwikPrns } from "personal-rns/qwik";
import { usePrns as useReactPrns } from "personal-rns/react";
import { usePrns as useSolidPrns } from "personal-rns/solid";
import { getPrnsContext } from "personal-rns/svelte";
import { usePrns as useVuePrns } from "personal-rns/vue";

const clientBoundaries = [
  ["personal-rns/qwik", useQwikPrns],
  ["personal-rns/react", useReactPrns],
  ["personal-rns/solid", useSolidPrns],
  ["personal-rns/svelte", getPrnsContext],
  ["personal-rns/vue", useVuePrns],
];

test("framework adapters reject server-side hook use with a typed boundary error", () => {
  for (const [adapter, invoke] of clientBoundaries) {
    assert.throws(invoke, (error) =>
      error instanceof Error &&
      error.name === "PrnsClientBoundaryRequiredError" &&
      error.message.includes(adapter)
    );
  }
});
