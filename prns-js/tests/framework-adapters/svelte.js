import { mount, unmount } from "svelte";
import SvelteConsumer from "./SvelteConsumer.svelte";

export function mountSvelteAdapter(prns) {
  const target = document.getElementById("svelte-target");
  if (target === null) {
    throw new Error("framework adapter test target svelte-target is missing");
  }
  const component = mount(SvelteConsumer, { target, props: { prns } });
  return () => unmount(component);
}
