import { component$ } from "@builder.io/qwik";
import type { NoSerialize } from "@builder.io/qwik";
import { prnsView } from "personal-rns/browser";
import type { Prns } from "personal-rns/browser";
import {
  PrnsProvider,
  usePrnsProjection,
} from "personal-rns/qwik";

export type QwikAdapterTestProps = {
  readonly prns: NoSerialize<Prns>;
};

export const QwikAdapterTest = component$<QwikAdapterTestProps>((props) =>
  <PrnsProvider prns={props.prns}>
    <QwikConsumer />
  </PrnsProvider>
);

export const QwikConsumer = component$(() => {
  const lifecycle = usePrnsProjection(prnsView("Lifecycle"));
  return <output
    data-framework="qwik"
    data-state={lifecycle.value.tag}
  >{lifecycle.value.tag}</output>;
});
