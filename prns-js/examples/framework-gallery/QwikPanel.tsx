import { component$ } from "@builder.io/qwik";
import type { JSXChildren, NoSerialize } from "@builder.io/qwik";
import { prnsView } from "personal-rns/browser";
import type { Prns } from "personal-rns/browser";
import {
  PrnsProvider,
  usePrnsProjection,
} from "personal-rns/qwik";

export type QwikGalleryProps = {
  readonly prns: NoSerialize<Prns>;
  readonly children?: JSXChildren;
};

export const QwikGallery = component$<QwikGalleryProps>((props) =>
  <PrnsProvider prns={props.prns}>
    <QwikPanel />
    {props.children}
  </PrnsProvider>
);

export const QwikPanel = component$(() => {
  const lifecycle = usePrnsProjection(prnsView("Lifecycle"));
  const interfaces = usePrnsProjection(prnsView("Interfaces"));
  const routes = usePrnsProjection(prnsView("Routes"));
  const links = usePrnsProjection(prnsView("Links"));
  const diagnostics = usePrnsProjection(
    prnsView("Diagnostics", { maximumEvents: 32 }),
  );
  return <article
    class="panel"
    data-diagnostics={diagnostics.value.length}
    data-framework="qwik"
    data-interfaces={interfaces.value.length}
    data-links={links.value.length}
    data-routes={routes.value.length}
  >
    <h2>Qwik</h2>
    <dl>
      <dt>Lifecycle</dt><dd>{lifecycle.value.tag}</dd>
      <dt>Interfaces</dt><dd>{interfaces.value.length}</dd>
      <dt>Routes</dt><dd>{routes.value.length}</dd>
      <dt>Links</dt><dd>{links.value.length}</dd>
      <dt>Diagnostics</dt><dd>{diagnostics.value.length}</dd>
    </dl>
  </article>;
});
