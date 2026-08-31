<script>
  import { prnsView } from "personal-rns/browser";
  import { prnsReadable, setPrnsContext } from "personal-rns/svelte";

  let { prns } = $props();
  setPrnsContext(prns);
  const lifecycle = prnsReadable(prnsView("Lifecycle"));
</script>

<output data-framework="svelte" data-state={$lifecycle.tag}>{$lifecycle.tag}</output>
