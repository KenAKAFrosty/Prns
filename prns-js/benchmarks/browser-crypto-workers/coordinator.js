import { Tag, match } from "../../dist/casework.js";
import {
  WebCryptoResourceDigester,
  WebCryptoResourceOpener,
  WebCryptoResourceSealer,
} from "../../dist/browser/resource_crypto.js";
import {
  WebCryptoResourceWorkerPool,
} from "../../dist/browser/resource_crypto_pool.js";

const MEBIBYTE = 1_024 * 1_024;
const SAMPLE_REPETITIONS = 5;
const scenarios = [
  { bytes: MEBIBYTE, jobs: 8 },
  { bytes: 4 * MEBIBYTE, jobs: 4 },
];
let latencyProbe;

self.addEventListener("message", ({ data }) => {
  match(data, {
    Initialize: ({ probe }) => {
      latencyProbe = new CoordinatorLatencyProbe(probe);
      void run().then(
        (result) => self.postMessage(Tag("Completed", result)),
        (error) => self.postMessage(Tag("Failed", {
          detail: String(error?.stack ?? error),
        })),
      );
    },
  });
});

async function run() {
  const inline = new InlineResourceCrypto();
  const oneWorker = new WebCryptoResourceWorkerPool(1);
  const twoWorkers = new WebCryptoResourceWorkerPool(2);
  try {
    const [oneReady, twoReady] = await Promise.all([
      oneWorker.ready(),
      twoWorkers.ready(),
    ]);
    if (oneReady.tag !== "Ready" || oneReady.data.workers !== 1) {
      throw new Error("one-worker crypto pool did not start exactly one Worker");
    }
    if (twoReady.tag !== "Ready" || twoReady.data.workers !== 2) {
      throw new Error("two-worker crypto pool did not start exactly two Workers");
    }
    const boundedAdmission = await exerciseBoundedAdmission(oneWorker);
    const configurations = [
      { name: "Inline", executor: inline },
      { name: "OneWorker", executor: new PoolResourceCrypto(oneWorker) },
      { name: "TwoWorkers", executor: new PoolResourceCrypto(twoWorkers) },
    ];
    for (const configuration of configurations) {
      await exercise(configuration.executor, 64 * 1_024, 2, 2, 0);
    }
    const results = [];
    for (const scenario of scenarios) {
      for (const lanes of [1, 2]) {
        const samples = new Map(
          configurations.map(({ name }) => [name, []]),
        );
        for (let repetition = 0; repetition < SAMPLE_REPETITIONS; repetition += 1) {
          let expectedChecksum;
          for (let offset = 0; offset < configurations.length; offset += 1) {
            const configuration = configurations[
              (offset + repetition) % configurations.length
            ];
            const measured = await measure(
              configuration.executor,
              scenario.bytes,
              scenario.jobs,
              lanes,
              repetition,
            );
            if (expectedChecksum === undefined) {
              expectedChecksum = measured.checksum;
            } else if (measured.checksum !== expectedChecksum) {
              throw new Error("crypto configurations produced different plaintext or digest bytes");
            }
            samples.get(configuration.name).push(measured);
          }
        }
        const inlineMedian = median(
          samples.get("Inline").map(({ elapsedMillis }) => elapsedMillis),
        );
        for (const configuration of configurations) {
          const configurationSamples = samples.get(configuration.name);
          const elapsedMillis = median(
            configurationSamples.map((sample) => sample.elapsedMillis),
          );
          const sourceBytes = scenario.bytes * scenario.jobs;
          results.push({
            configuration: configuration.name,
            resourceBytes: scenario.bytes,
            jobs: scenario.jobs,
            lanes,
            elapsedMillis,
            mebibytesPerSecond: sourceBytes / (elapsedMillis / 1_000) / MEBIBYTE,
            speedupOverInline: inlineMedian / elapsedMillis,
            medianCoordinatorP95Millis: median(
              configurationSamples.map((sample) => sample.coordinatorLatency.p95Millis),
            ),
            worstCoordinatorLatencyMillis: Math.max(
              ...configurationSamples.map((sample) => sample.coordinatorLatency.maximumMillis),
            ),
            medianProbeSamples: median(
              configurationSamples.map((sample) => sample.coordinatorLatency.samples),
            ),
            checksum: configurationSamples[0].checksum,
          });
        }
      }
    }
    return {
      userAgent: navigator.userAgent,
      hardwareConcurrency: navigator.hardwareConcurrency,
      workerReadiness: {
        one: oneReady.data.workers,
        two: twoReady.data.workers,
      },
      boundedAdmission,
      results,
    };
  } finally {
    oneWorker.close();
    twoWorkers.close();
  }
}

async function exerciseBoundedAdmission(pool) {
  const settlements = await Promise.all(
    Array.from(
      { length: 17 },
      (_, index) => pool.donateSeal(transaction(64 * 1_024, index, 0, 91).seal),
    ),
  );
  const sealed = settlements.filter((settlement) => settlement.tag === "Sealed").length;
  const busy = settlements.filter((settlement) => settlement.tag === "Busy").length;
  if (sealed !== 16 || busy !== 1) {
    throw new Error(`bounded admission produced ${sealed} sealed and ${busy} busy outcomes`);
  }
  const recovered = await pool.donateSeal(transaction(64 * 1_024, 18, 0, 91).seal);
  if (recovered.tag !== "Sealed") {
    throw new Error(`bounded admission did not recover: ${recovered.tag}`);
  }
  return { admitted: sealed, busy, recovered: true };
}

async function measure(executor, bytes, jobs, lanes, repetition) {
  if (latencyProbe === undefined) {
    throw new Error("coordinator latency probe was not initialized");
  }
  await yieldTask();
  await latencyProbe.start();
  const started = performance.now();
  const checksum = await exercise(executor, bytes, jobs, lanes, repetition + 1);
  const elapsedMillis = performance.now() - started;
  const coordinatorLatency = await latencyProbe.stop();
  return {
    elapsedMillis,
    coordinatorLatency,
    checksum,
  };
}

async function exercise(executor, bytes, jobs, lanes, seed) {
  const work = Array.from({ length: lanes }, () => []);
  for (let index = 0; index < jobs; index += 1) {
    work[index % lanes].push(transaction(bytes, index, index % lanes, seed));
  }
  const laneChecksums = await Promise.all(
    work.map(async (lane) => {
      let checksum = 0;
      for (const job of lane) {
        checksum = (checksum + await executor.transact(job)) >>> 0;
      }
      return checksum;
    }),
  );
  return laneChecksums.reduce((sum, checksum) => (sum + checksum) >>> 0, 0);
}

function transaction(bytes, index, lane, seed) {
  const linkId = Uint8Array.from(
    { length: 16 },
    (_, byte) => (byte * 17 + lane * 31 + 1) & 0xff,
  );
  const signingKey = Uint8Array.from(
    { length: 32 },
    (_, byte) => (byte * 7 + lane * 13 + 5) & 0xff,
  );
  const encryptionKey = Uint8Array.from(
    { length: 32 },
    (_, byte) => (255 - byte * 5 - lane * 11) & 0xff,
  );
  const plaintext = new Uint8Array(bytes);
  plaintext.fill((index * 29 + seed * 37 + 0xa5) & 0xff);
  plaintext.set(Uint8Array.of(1, 2, 3, (index + seed) & 0xff));
  return {
    seal: {
      linkId,
      plaintext,
      signingKey,
      encryptionKey,
      sealIv: Uint8Array.from(
        { length: 16 },
        (_, byte) => (byte * 19 + index * 23 + seed) & 0xff,
      ),
    },
    salt: Uint8Array.of(9, 8, 7, (6 + index + seed) & 0xff),
    openKeys: {
      linkId: linkId.slice(),
      signingKey: signingKey.slice(),
      encryptionKey: encryptionKey.slice(),
    },
    expectedFirst: plaintext[0],
    expectedLast: plaintext[plaintext.length - 1],
  };
}

class InlineResourceCrypto {
  #sealer = new WebCryptoResourceSealer();
  #opener = new WebCryptoResourceOpener();
  #digester = new WebCryptoResourceDigester();

  async transact(transaction) {
    const sealed = await this.#sealer.seal(transaction.seal);
    const digests = await this.#digester.digest(
      transaction.seal.plaintext,
      transaction.salt,
    );
    const opened = await this.#opener.open({
      ...transaction.openKeys,
      sealed,
    });
    return openedChecksum(opened, digests, transaction);
  }
}

class PoolResourceCrypto {
  #pool;

  constructor(pool) {
    this.#pool = pool;
  }

  async transact(transaction) {
    const sealed = await this.#pool.donateSeal(transaction.seal);
    if (sealed.tag !== "Sealed") {
      throw new Error(`resource seal returned ${sealed.tag}`);
    }
    const digested = await this.#pool.donateDigest(
      sealed.data.plaintext,
      transaction.salt,
    );
    if (digested.tag !== "Digested") {
      throw new Error(`resource digest returned ${digested.tag}`);
    }
    const opened = await this.#pool.donateOpen({
      ...transaction.openKeys,
      sealed: sealed.data.sealed,
    });
    const inlineOutcome = match(opened, {
      Opened: (plaintext) => Tag("Opened", plaintext),
      Refused: () => Tag("Refused"),
      Busy: () => {
        throw new Error("resource open pool was busy");
      },
      Unavailable: () => {
        throw new Error("resource open pool became unavailable");
      },
      Failed: ({ detail }) => {
        throw new Error(`resource open failed: ${detail}`);
      },
    });
    return openedChecksum(
      inlineOutcome,
      digested.data,
      transaction,
    );
  }
}

function openedChecksum(opened, digests, transaction) {
  if (opened.tag !== "Opened") {
    throw new Error("resource token was refused");
  }
  const plaintext = opened.data;
  if (
    plaintext[0] !== transaction.expectedFirst ||
    plaintext[plaintext.length - 1] !== transaction.expectedLast
  ) {
    throw new Error("opened resource bytes did not match their input");
  }
  return (
    plaintext.byteLength +
    plaintext[0] +
    plaintext[plaintext.length - 1] +
    digests.hash[0] +
    digests.proof[0]
  ) >>> 0;
}

function median(values) {
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.floor(ordered.length / 2)];
}

function yieldTask() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

class CoordinatorLatencyProbe {
  #port;
  #nextId = 1;
  #started = new Map();
  #results = new Map();
  #activeId;

  constructor(port) {
    this.#port = port;
    port.addEventListener("message", ({ data }) => {
      match(data, {
        Ping: ({ id, sequence }) => {
          port.postMessage(Tag("Pong", { id, sequence }));
        },
        ProbeStarted: ({ id }) => {
          const settle = this.#started.get(id);
          if (settle === undefined) {
            throw new Error("latency probe started an unknown measurement");
          }
          this.#started.delete(id);
          settle();
        },
        ProbeResult: (result) => {
          const settle = this.#results.get(result.id);
          if (settle === undefined) {
            throw new Error("latency probe settled an unknown measurement");
          }
          this.#results.delete(result.id);
          settle(result);
        },
      });
    });
    port.start();
  }

  start() {
    if (this.#activeId !== undefined) {
      throw new Error("latency probe measurement is already active");
    }
    const id = this.#nextId;
    this.#nextId += 1;
    this.#activeId = id;
    return new Promise((settle) => {
      this.#started.set(id, settle);
      this.#port.postMessage(Tag("BeginProbe", { id }));
    });
  }

  stop() {
    const id = this.#activeId;
    if (id === undefined) {
      throw new Error("latency probe measurement is not active");
    }
    this.#activeId = undefined;
    return new Promise((settle) => {
      this.#results.set(id, settle);
      this.#port.postMessage(Tag("EndProbe", { id }));
    });
  }
}
