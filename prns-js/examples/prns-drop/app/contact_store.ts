import { Tag } from "personal-rns/browser";
import type { Tag as Tagged } from "personal-rns/browser";
import type { StoredDropContact } from "./model.js";
import {
  exportDropContactCode,
  parseDropContactCode,
} from "./protocol.js";

export type DropContactStoreLoadOutcome =
  | Tagged<"Loaded", readonly StoredDropContact[]>
  | Tagged<"Unavailable", { readonly detail: string }>;

export type DropContactStoreSaveOutcome =
  | Tagged<"Saved">
  | Tagged<"Unavailable", { readonly detail: string }>;

export interface DropContactStore {
  load(): Promise<DropContactStoreLoadOutcome>;
  save(contacts: readonly StoredDropContact[]): Promise<DropContactStoreSaveOutcome>;
}

export class BrowserDropContactStore implements DropContactStore {
  readonly #key: string;

  constructor(root: string = "prns.drop") {
    const selected = root.trim();
    if (selected.length === 0) {
      throw new Error("Drop contact storage root must not be empty");
    }
    this.#key = `${selected}.contacts.v1`;
  }

  async load(): Promise<DropContactStoreLoadOutcome> {
    try {
      const encoded = globalThis.localStorage.getItem(this.#key);
      if (encoded === null) {
        return Tag("Loaded", []);
      }
      const raw: unknown = JSON.parse(encoded);
      if (!Array.isArray(raw) || raw.some((value) => typeof value !== "string")) {
        return Tag("Unavailable", { detail: "stored contacts are not a contact-code array" });
      }
      const contacts: StoredDropContact[] = [];
      for (const code of raw) {
        const parsed = parseDropContactCode(code);
        if (parsed.tag !== "Parsed") {
          return Tag("Unavailable", { detail: parsed.data.detail });
        }
        contacts.push(parsed.data);
      }
      return Tag("Loaded", contacts);
    } catch (error: unknown) {
      return Tag("Unavailable", { detail: describeUnknown(error) });
    }
  }

  async save(contacts: readonly StoredDropContact[]): Promise<DropContactStoreSaveOutcome> {
    try {
      globalThis.localStorage.setItem(
        this.#key,
        JSON.stringify(contacts.map(exportDropContactCode)),
      );
      return Tag("Saved");
    } catch (error: unknown) {
      return Tag("Unavailable", { detail: describeUnknown(error) });
    }
  }
}

function describeUnknown(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
