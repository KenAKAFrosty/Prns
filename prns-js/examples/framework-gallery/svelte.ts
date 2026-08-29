import { mount } from "svelte";
import SveltePanel from "./SveltePanel.svelte";

const target = document.getElementById("svelte-panel");
if (target === null) {
  throw new Error("gallery element svelte-panel is missing");
}
mount(SveltePanel, {
  target,
});
