import { Tag, match } from "../../dist/casework.js";
import initPortableWasm, {
  profileEd25519Sign,
  profileEd25519Vector,
  profileX25519,
  profileX25519Vector,
} from "/prns-wasm/smoke/pkg/prns_wasm.js";

const SAMPLE_REPETITIONS = 5;
const BATCHED_OPERATIONS = 4_096;
const SINGLE_OPERATIONS = 512;

export async function measurePortableWasmWorkers(latencyProbe) {
  const inlineStarted = performance.now();
  await initPortableWasm();
  const inlineStartupMillis = performance.now() - inlineStarted;
  const expectedVectors = {
    ed25519: profileEd25519Vector(),
    x25519: profileX25519Vector(),
  };
  requireEqualBytes(
    expectedVectors.ed25519,
    bytesFromHex(
      "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a" +
      "36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
    ),
    "inline Ed25519",
  );
  requireEqualBytes(
    expectedVectors.x25519,
    bytesFromHex("1fdc192faa0212a9aae7bb4f41b580227fd5ad3e5d777faae230dfe973f3e805"),
    "inline X25519",
  );

  const workersStarted = performance.now();
  const workers = [
    new PortableWasmWorkerClient(1, expectedVectors),
    new PortableWasmWorkerClient(2, expectedVectors),
    new PortableWasmWorkerClient(3, expectedVectors),
    new PortableWasmWorkerClient(4, expectedVectors),
  ];
  await Promise.all(workers.map((worker) => worker.ready()));
  const workerStartupMillis = performance.now() - workersStarted;

  const configurations = [
    new InlinePortableWasm(),
    new WorkerPortableWasm("OneWorker", workers.slice(0, 1)),
    new WorkerPortableWasm("TwoWorkers", workers.slice(0, 2)),
    new WorkerPortableWasm("ThreeWorkers", workers.slice(0, 3)),
    new WorkerPortableWasm("FourWorkers", workers),
  ];
  try {
    for (const configuration of configurations) {
      await configuration.perform("Batched", "Ed25519Sign", 32);
      await configuration.perform("Batched", "X25519", 32);
      await configuration.perform("Singles", "Ed25519Sign", 4);
      await configuration.perform("Singles", "X25519", 4);
    }
    const results = [];
    for (const operation of ["Ed25519Sign", "X25519"]) {
      for (const mode of ["Batched", "Singles"]) {
        const operations = mode === "Batched"
          ? BATCHED_OPERATIONS
          : SINGLE_OPERATIONS;
        const samples = new Map(configurations.map(({ name }) => [name, []]));
        for (let repetition = 0; repetition < SAMPLE_REPETITIONS; repetition += 1) {
          let expectedChecksum;
          for (let offset = 0; offset < configurations.length; offset += 1) {
            const configuration = configurations[
              (offset + repetition) % configurations.length
            ];
            const measured = await measureConfiguration(
              latencyProbe,
              configuration,
              mode,
              operation,
              operations,
            );
            if (expectedChecksum === undefined) {
              expectedChecksum = measured.checksum;
            } else if (measured.checksum !== expectedChecksum) {
              throw new Error(`${operation} ${mode} configurations produced different checksums`);
            }
            samples.get(configuration.name).push(measured);
          }
        }
        const inlineMillis = median(
          samples.get("Inline").map(({ elapsedMillis }) => elapsedMillis),
        );
        for (const configuration of configurations) {
          const configurationSamples = samples.get(configuration.name);
          const elapsedMillis = median(
            configurationSamples.map((sample) => sample.elapsedMillis),
          );
          results.push({
            operation,
            mode,
            configuration: configuration.name,
            workers: configuration.workers,
            operations,
            elapsedMillis,
            operationsPerSecond: operations / (elapsedMillis / 1_000),
            speedupOverInline: inlineMillis / elapsedMillis,
            medianCoordinatorP95Millis: median(
              configurationSamples.map((sample) => sample.coordinatorLatency.p95Millis),
            ),
            worstCoordinatorLatencyMillis: Math.max(
              ...configurationSamples.map((sample) => sample.coordinatorLatency.maximumMillis),
            ),
            checksum: configurationSamples[0].checksum,
          });
        }
      }
    }
    return {
      inlineStartupMillis,
      workerStartupMillis,
      startedWorkers: workers.length,
      results,
    };
  } finally {
    for (const worker of workers) {
      worker.close();
    }
  }
}

async function measureConfiguration(
  latencyProbe,
  configuration,
  mode,
  operation,
  operations,
) {
  await yieldTask();
  await latencyProbe.start();
  const started = performance.now();
  const checksum = await configuration.perform(mode, operation, operations);
  const elapsedMillis = performance.now() - started;
  const coordinatorLatency = await latencyProbe.stop();
  return { elapsedMillis, coordinatorLatency, checksum };
}

class InlinePortableWasm {
  name = "Inline";
  workers = 0;

  async perform(mode, operation, operations) {
    if (mode === "Batched") {
      return runInline(operation, operations);
    }
    let checksum = 0;
    for (let index = 0; index < operations; index += 1) {
      checksum = (checksum + runInline(operation, 1)) >>> 0;
    }
    return checksum;
  }
}

class WorkerPortableWasm {
  name;
  workers;
  #clients;

  constructor(name, clients) {
    this.name = name;
    this.workers = clients.length;
    this.#clients = clients;
  }

  async perform(mode, operation, operations) {
    const distribution = distribute(operations, this.#clients.length);
    const checksums = await Promise.all(
      this.#clients.map(async (client, index) => {
        const assigned = distribution[index];
        if (mode === "Batched") {
          return client.run(operation, assigned);
        }
        let checksum = 0;
        for (let job = 0; job < assigned; job += 1) {
          checksum = (checksum + await client.run(operation, 1)) >>> 0;
        }
        return checksum;
      }),
    );
    return checksums.reduce((sum, checksum) => (sum + checksum) >>> 0, 0);
  }
}

class PortableWasmWorkerClient {
  #worker;
  #nextId = 1;
  #pending = new Map();
  #ready;
  #settleReady;
  #refuseReady;
  #expectedVectors;

  constructor(index, expectedVectors) {
    this.#expectedVectors = expectedVectors;
    this.#ready = new Promise((settle, refuse) => {
      this.#settleReady = settle;
      this.#refuseReady = refuse;
    });
    this.#worker = new Worker("./portable_wasm_worker.js", {
      type: "module",
      name: `prns-portable-crypto-${index}`,
    });
    this.#worker.addEventListener("message", ({ data }) => this.#receive(data));
    this.#worker.addEventListener("error", (event) => {
      this.#failAll(event.message || "portable WASM worker failed");
    });
    this.#worker.addEventListener("messageerror", () => {
      this.#failAll("portable WASM worker emitted an unreadable response");
    });
  }

  ready() {
    return this.#ready;
  }

  run(operation, iterations) {
    const id = this.#nextId;
    this.#nextId += 1;
    return new Promise((settle, refuse) => {
      this.#pending.set(id, { settle, refuse });
      this.#worker.postMessage(Tag("Run", { id, operation, iterations }));
    });
  }

  close() {
    this.#failAll("portable WASM worker closed");
    this.#worker.terminate();
  }

  #receive(response) {
    match(response, {
      Ready: ({ ed25519Vector, x25519Vector }) => {
        try {
          requireEqualBytes(
            ed25519Vector,
            this.#expectedVectors.ed25519,
            "worker Ed25519",
          );
          requireEqualBytes(
            x25519Vector,
            this.#expectedVectors.x25519,
            "worker X25519",
          );
          this.#settleReady();
        } catch (error) {
          this.#refuseReady(error);
        }
      },
      Completed: ({ id, checksum }) => {
        const pending = this.#pending.get(id);
        if (pending === undefined) {
          throw new Error("portable WASM worker completed an unknown job");
        }
        this.#pending.delete(id);
        pending.settle(checksum);
      },
      Failed: ({ id, detail }) => {
        const pending = this.#pending.get(id);
        if (pending === undefined) {
          throw new Error("portable WASM worker failed an unknown job");
        }
        this.#pending.delete(id);
        pending.refuse(new Error(detail));
      },
    });
  }

  #failAll(detail) {
    const error = new Error(detail);
    this.#refuseReady(error);
    for (const pending of this.#pending.values()) {
      pending.refuse(error);
    }
    this.#pending.clear();
  }
}

function runInline(operation, iterations) {
  return operation === "Ed25519Sign"
    ? profileEd25519Sign(iterations)
    : profileX25519(iterations);
}

function distribute(total, lanes) {
  const base = Math.floor(total / lanes);
  const remainder = total % lanes;
  return Array.from(
    { length: lanes },
    (_, index) => base + (index < remainder ? 1 : 0),
  );
}

function bytesFromHex(value) {
  return Uint8Array.from(
    { length: value.length / 2 },
    (_, index) => Number.parseInt(value.slice(index * 2, index * 2 + 2), 16),
  );
}

function requireEqualBytes(actual, expected, name) {
  if (
    actual.length !== expected.length ||
    !actual.every((byte, index) => byte === expected[index])
  ) {
    throw new Error(`${name} did not match its Prns vector`);
  }
}

function median(values) {
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.floor(ordered.length / 2)];
}

function yieldTask() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}
