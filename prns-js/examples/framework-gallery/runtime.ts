import type { Prns } from "personal-rns/browser";

export type FrameworkGalleryRuntime = {
  readonly prns: Prns;
};

declare global {
  interface Window {
    prnsFrameworkGallery?: FrameworkGalleryRuntime;
  }
}

export function galleryRuntime(): FrameworkGalleryRuntime {
  const runtime = globalThis.window.prnsFrameworkGallery;
  if (runtime === undefined) {
    throw new Error("Prns framework gallery runtime is unavailable");
  }
  return runtime;
}
