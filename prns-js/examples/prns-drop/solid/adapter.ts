import { from } from "solid-js";
import type { Accessor } from "solid-js";
import type { DropSnapshot } from "../app/model.js";
import type { PrnsDrop } from "../app/drop.js";

export function createDropSnapshot(drop: PrnsDrop): Accessor<DropSnapshot> {
  return from(
    (set) => drop.subscribe((snapshot) => set(() => snapshot)),
    drop.snapshot(),
  );
}
