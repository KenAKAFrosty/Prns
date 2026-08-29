<script lang="ts">
  import { prnsView } from "personal-rns/browser";
  import { prnsReadable, setPrnsContext } from "personal-rns/svelte";
  import { galleryRuntime } from "./runtime.js";

  setPrnsContext(galleryRuntime().prns);
  const lifecycle = prnsReadable(prnsView("Lifecycle"));
  const interfaces = prnsReadable(prnsView("Interfaces"));
  const routes = prnsReadable(prnsView("Routes"));
  const links = prnsReadable(prnsView("Links"));
  const diagnostics = prnsReadable(
    prnsView("Diagnostics", { maximumEvents: 32 }),
  );
</script>

<article
  class="panel"
  data-diagnostics={$diagnostics.length}
  data-framework="svelte"
  data-interfaces={$interfaces.length}
  data-links={$links.length}
  data-routes={$routes.length}
>
  <h2>Svelte</h2>
  <dl>
    <dt>Lifecycle</dt><dd>{$lifecycle.tag}</dd>
    <dt>Interfaces</dt><dd>{$interfaces.length}</dd>
    <dt>Routes</dt><dd>{$routes.length}</dd>
    <dt>Links</dt><dd>{$links.length}</dd>
    <dt>Diagnostics</dt><dd>{$diagnostics.length}</dd>
  </dl>
</article>
