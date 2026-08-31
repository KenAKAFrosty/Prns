import { Tag } from "personal-rns/browser";

export class FakePrns {
  #revision = 1n;
  #deliveries = 0;
  #projections = new Map([
    ["Lifecycle", new FakeProjection(Tag("Starting"), () => this.#deliveries += 1)],
    ["Interfaces", new FakeProjection(Object.freeze([]), () => this.#deliveries += 1)],
    ["Routes", new FakeProjection(Object.freeze([]), () => this.#deliveries += 1)],
    ["Links", new FakeProjection(Object.freeze([]), () => this.#deliveries += 1)],
    ["Diagnostics", new FakeProjection(Object.freeze([]), () => this.#deliveries += 1)],
  ]);

  projection(view) {
    const projection = this.#projections.get(view.tag);
    if (projection === undefined) {
      throw new TypeError(`unknown test projection ${view.tag}`);
    }
    return projection;
  }

  publishLifecycle(lifecycle) {
    this.#revision += 1n;
    this.#projections.get("Lifecycle").replace({
      revision: this.#revision,
      value: lifecycle,
    });
  }

  execute() {
    return Promise.resolve(Tag("Failed", Tag("NodeStopped")));
  }

  get activeSubscriptions() {
    let total = 0;
    for (const projection of this.#projections.values()) {
      total += projection.subscriptions;
    }
    return total;
  }

  get deliveries() {
    return this.#deliveries;
  }
}

class FakeProjection {
  #snapshot;
  #delivered;
  #listeners = new Set();

  constructor(value, delivered) {
    this.#snapshot = { revision: 1n, value };
    this.#delivered = delivered;
  }

  latest() {
    return this.#snapshot;
  }

  synchronize() {
    return Promise.resolve(Tag("Synchronized", this.#snapshot));
  }

  subscribe(changed) {
    this.#listeners.add(changed);
    let subscribed = true;
    return () => {
      if (!subscribed) {
        return;
      }
      subscribed = false;
      this.#listeners.delete(changed);
    };
  }

  replace(snapshot) {
    this.#snapshot = snapshot;
    for (const changed of this.#listeners) {
      this.#delivered();
      changed();
    }
  }

  get subscriptions() {
    return this.#listeners.size;
  }
}
