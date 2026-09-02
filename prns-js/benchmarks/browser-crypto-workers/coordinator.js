import { Tag, match } from "../../dist/casework.js";
import {
  WebCryptoResourceDigester,
  WebCryptoResourceOpener,
  WebCryptoResourceSealer,
} from "../../dist/browser/resource_crypto.js";
import {
  WebCryptoWorkerPool,
} from "../../dist/browser/crypto_pool.js";
import {
  WebCryptoEd25519Signer,
  WebCryptoEd25519Verifier,
  WebCryptoHkdfSha256Deriver,
  WebCryptoX25519Deriver,
} from "../../dist/browser/protocol_crypto.js";
import { measurePortableWasmWorkers } from "./portable_wasm_benchmark.js";
import { ConcurrentWebCryptoGateway } from "./concurrent_webcrypto_gateway.js";

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
  const oneWorker = new WebCryptoWorkerPool(1);
  const twoWorkers = new WebCryptoWorkerPool(2);
  const fourWorkers = new WebCryptoWorkerPool(4);
  const gatewayOne = new ConcurrentWebCryptoGateway(1);
  const gatewayTwo = new ConcurrentWebCryptoGateway(2);
  const gatewayFour = new ConcurrentWebCryptoGateway(4);
  try {
    const [oneReady, twoReady, fourReady, gatewayOneReady, gatewayTwoReady, gatewayFourReady] = await Promise.all([
      oneWorker.ready(),
      twoWorkers.ready(),
      fourWorkers.ready(),
      gatewayOne.ready(),
      gatewayTwo.ready(),
      gatewayFour.ready(),
    ]);
    if (oneReady.tag !== "Ready" || oneReady.data.workers !== 1) {
      throw new Error("one-worker crypto pool did not start exactly one Worker");
    }
    if (twoReady.tag !== "Ready" || twoReady.data.workers !== 2) {
      throw new Error("two-worker crypto pool did not start exactly two Workers");
    }
    if (fourReady.tag !== "Ready" || fourReady.data.workers !== 4) {
      throw new Error("four-worker crypto pool did not start exactly four Workers");
    }
    for (const [maximumInFlight, readiness] of [
      [1, gatewayOneReady],
      [2, gatewayTwoReady],
      [4, gatewayFourReady],
    ]) {
      if (
        readiness.tag !== "Ready" ||
        readiness.data.workers !== 1 ||
        readiness.data.maximumInFlight !== maximumInFlight
      ) {
        throw new Error(`WebCrypto gateway did not start with concurrency ${maximumInFlight}`);
      }
    }
    const protocolCrypto = await exerciseProtocolCrypto(twoWorkers, twoReady.data.compatibility);
    const protocolPerformance = await measureProtocolCrypto(
      twoWorkers,
      gatewayOne,
      gatewayTwo,
      gatewayFour,
    );
    const mixedResourceExecutor = new PoolResourceCrypto(twoWorkers);
    const portableWasmWorkers = await measurePortableWasmWorkers(latencyProbe, {
      resourceBytes: 4 * MEBIBYTE,
      jobs: 4,
      lanes: 2,
      perform: (seed) => exercise(
        mixedResourceExecutor,
        4 * MEBIBYTE,
        4,
        2,
        seed,
      ),
    });
    const boundedAdmission = await exerciseBoundedAdmission(oneWorker);
    const configurations = [
      { name: "Inline", executor: inline },
      { name: "OneWorker", executor: new PoolResourceCrypto(oneWorker) },
      { name: "TwoWorkers", executor: new PoolResourceCrypto(twoWorkers) },
      { name: "FourWorkers", executor: new PoolResourceCrypto(fourWorkers) },
      { name: "Gateway1", executor: new PoolResourceCrypto(gatewayOne) },
      { name: "Gateway2", executor: new PoolResourceCrypto(gatewayTwo) },
      { name: "Gateway4", executor: new PoolResourceCrypto(gatewayFour) },
    ];
    for (const configuration of configurations) {
      await exercise(configuration.executor, 64 * 1_024, 2, 2, 0);
    }
    const results = [];
    for (const scenario of scenarios) {
      for (const lanes of [1, 2, 4]) {
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
        four: fourReady.data.workers,
      },
      gatewayReadiness: {
        one: {
          configured: gatewayOneReady.data.maximumInFlight,
          observed: gatewayOne.maximumObservedInFlight(),
        },
        two: {
          configured: gatewayTwoReady.data.maximumInFlight,
          observed: gatewayTwo.maximumObservedInFlight(),
        },
        four: {
          configured: gatewayFourReady.data.maximumInFlight,
          observed: gatewayFour.maximumObservedInFlight(),
        },
      },
      boundedAdmission,
      protocolCrypto,
      protocolPerformance,
      portableWasmWorkers,
      results,
    };
  } finally {
    oneWorker.close();
    twoWorkers.close();
    fourWorkers.close();
    gatewayOne.close();
    gatewayTwo.close();
    gatewayFour.close();
  }
}

async function exerciseProtocolCrypto(pool, compatibility) {
  for (const capability of Object.values(compatibility)) {
    if (capability.tag !== "Compatible") {
      throw new Error(`browser protocol crypto is unavailable: ${capability.data.detail}`);
    }
  }
  const message = new TextEncoder().encode("sign-this");
  const expectedSignature = bytesFromHex(
    "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a" +
    "36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
  );
  const signingSecret = new Uint8Array(32).fill(0x11);
  const signed = await pool.donateEd25519Sign(signingSecret, message);
  if (signed.tag !== "Signed") {
    throw new Error(`Ed25519 sign returned ${signed.tag}`);
  }
  requireEqualBytes(signed.data.signature, expectedSignature, "Ed25519 signature");
  if (signingSecret.byteLength !== 0) {
    throw new Error("Ed25519 signing secret ownership was not transferred");
  }
  const publicKey = bytesFromHex(
    "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737",
  );
  const verified = await pool.donateEd25519Verify(
    publicKey,
    new TextEncoder().encode("sign-this"),
    expectedSignature,
  );
  if (verified.tag !== "Valid") {
    throw new Error(`Ed25519 golden signature returned ${verified.tag}`);
  }
  const refused = await pool.donateEd25519Verify(
    bytesFromHex("d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"),
    new TextEncoder().encode("sign-thus"),
    bytesFromHex(
      "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a" +
      "36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
    ),
  );
  if (refused.tag !== "Invalid") {
    throw new Error(`Ed25519 altered message returned ${refused.tag}`);
  }
  const x25519Secret = new Uint8Array(32).fill(0x22);
  const derived = await pool.donateX25519Derive(
    x25519Secret,
    bytesFromHex("7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14"),
  );
  if (derived.tag !== "Derived") {
    throw new Error(`X25519 derive returned ${derived.tag}`);
  }
  requireEqualBytes(
    derived.data.sharedSecret,
    bytesFromHex("1fdc192faa0212a9aae7bb4f41b580227fd5ad3e5d777faae230dfe973f3e805"),
    "X25519 shared secret",
  );
  if (x25519Secret.byteLength !== 0) {
    throw new Error("X25519 secret ownership was not transferred");
  }
  const linkProofSecret = new Uint8Array(32).fill(0x22);
  const linkProof = await pool.donateLinkProofVerify(
    bytesFromHex("d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"),
    new TextEncoder().encode("sign-this"),
    bytesFromHex(
      "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a" +
      "36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
    ),
    linkProofSecret,
    bytesFromHex("7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14"),
  );
  if (linkProof.tag !== "Verified") {
    throw new Error(`link-proof composite returned ${linkProof.tag}`);
  }
  requireEqualBytes(
    linkProof.data.sharedSecret,
    bytesFromHex("1fdc192faa0212a9aae7bb4f41b580227fd5ad3e5d777faae230dfe973f3e805"),
    "link-proof shared secret",
  );
  if (linkProofSecret.byteLength !== 0) {
    throw new Error("link-proof secret ownership was not transferred");
  }
  const invalidLinkProofSecret = new Uint8Array(32).fill(0x22);
  const invalidLinkProof = await pool.donateLinkProofVerify(
    bytesFromHex("d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"),
    new TextEncoder().encode("sign-thus"),
    bytesFromHex(
      "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a" +
      "36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
    ),
    invalidLinkProofSecret,
    bytesFromHex("7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14"),
  );
  if (invalidLinkProof.tag !== "Invalid") {
    throw new Error(`altered link-proof composite returned ${invalidLinkProof.tag}`);
  }
  if (invalidLinkProofSecret.byteLength !== 0) {
    throw new Error("invalid link-proof secret ownership was not transferred");
  }
  const hkdf = await pool.donateHkdfSha256({
    inputKeyMaterial: new Uint8Array(32).fill(0x42),
    salt: new Uint8Array(16).fill(0x01),
    info: new TextEncoder().encode("context"),
    outputBytes: 64,
  });
  if (hkdf.tag !== "Derived") {
    throw new Error(`HKDF-SHA256 derive returned ${hkdf.tag}`);
  }
  requireEqualBytes(
    hkdf.data.keyMaterial,
    bytesFromHex(
      "d3a68f6569700c188c5a7c2bcd22c37e9757d022658f06b59753f7c079dcdb3a" +
      "82958b17892dbd30978719b5ba66787152ad0a0c7aeb4df49bce91d36c8915dd",
    ),
    "HKDF-SHA256 output",
  );
  return {
    compatibility,
    ed25519Sign: true,
    ed25519Verify: true,
    ed25519RejectsAlteredMessage: true,
    x25519: true,
    linkProofVerifyThenDerive: true,
    linkProofRejectsBeforeDerive: true,
    hkdfSha256: true,
    donatedSecretOwnership: true,
  };
}

async function measureProtocolCrypto(pool, gatewayOne, gatewayTwo, gatewayFour) {
  const configurations = [
    { name: "Inline", executor: new InlineProtocolCrypto() },
    { name: "TwoWorkers", executor: new PoolProtocolCrypto(pool) },
    { name: "Gateway1", executor: new PoolProtocolCrypto(gatewayOne) },
    { name: "Gateway2", executor: new PoolProtocolCrypto(gatewayTwo) },
    { name: "Gateway4", executor: new PoolProtocolCrypto(gatewayFour) },
  ];
  const operations = ["Ed25519Sign", "Ed25519Verify", "X25519", "HkdfSha256"];
  const operationCount = 128;
  const results = [];
  for (const operation of operations) {
    for (const configuration of configurations) {
      await exerciseProtocolOperation(configuration.executor, operation, 4, 2);
    }
    for (const lanes of [1, 2, 4]) {
      const samples = new Map(configurations.map(({ name }) => [name, []]));
      for (let repetition = 0; repetition < SAMPLE_REPETITIONS; repetition += 1) {
        let expectedChecksum;
        for (let offset = 0; offset < configurations.length; offset += 1) {
          const configuration = configurations[(offset + repetition) % configurations.length];
          const measured = await measureProtocolConfiguration(
            configuration.executor,
            operation,
            operationCount,
            lanes,
          );
          if (expectedChecksum === undefined) {
            expectedChecksum = measured.checksum;
          } else if (expectedChecksum !== measured.checksum) {
            throw new Error(`${operation} configurations produced different checksums`);
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
          configuration: configuration.name,
          operations: operationCount,
          lanes,
          elapsedMillis,
          operationsPerSecond: operationCount / (elapsedMillis / 1_000),
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
  return results;
}

async function measureProtocolConfiguration(executor, operation, operations, lanes) {
  if (latencyProbe === undefined) {
    throw new Error("coordinator latency probe was not initialized");
  }
  await yieldTask();
  await latencyProbe.start();
  const started = performance.now();
  const checksum = await exerciseProtocolOperation(executor, operation, operations, lanes);
  const elapsedMillis = performance.now() - started;
  const coordinatorLatency = await latencyProbe.stop();
  return { elapsedMillis, coordinatorLatency, checksum };
}

async function exerciseProtocolOperation(executor, operation, operations, lanes) {
  const work = Array.from({ length: lanes }, () => []);
  for (let index = 0; index < operations; index += 1) {
    work[index % lanes].push(index);
  }
  const checksums = await Promise.all(work.map(async (lane) => {
    let checksum = 0;
    for (const index of lane) {
      checksum = (checksum + await executor.perform(operation, index)) >>> 0;
    }
    return checksum;
  }));
  return checksums.reduce((sum, checksum) => (sum + checksum) >>> 0, 0);
}

class InlineProtocolCrypto {
  #signer = new WebCryptoEd25519Signer();
  #verifier = new WebCryptoEd25519Verifier();
  #x25519 = new WebCryptoX25519Deriver();
  #hkdf = new WebCryptoHkdfSha256Deriver();

  async perform(operation) {
    if (operation === "Ed25519Sign") {
      const signature = await this.#signer.sign(
        new Uint8Array(32).fill(0x11),
        new TextEncoder().encode("sign-this"),
      );
      return signature[0] + signature[signature.length - 1];
    }
    if (operation === "Ed25519Verify") {
      const verification = await this.#verifier.verify(
        bytesFromHex("d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"),
        new TextEncoder().encode("sign-this"),
        protocolSignature(),
      );
      if (verification.tag !== "Valid") {
        throw new Error(`inline Ed25519 verification returned ${verification.tag}`);
      }
      return 1;
    }
    if (operation === "X25519") {
      const shared = await this.#x25519.derive(
        new Uint8Array(32).fill(0x22),
        bytesFromHex("7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14"),
      );
      return shared[0] + shared[shared.length - 1];
    }
    const keyMaterial = await this.#hkdf.derive(protocolHkdfJob());
    return keyMaterial[0] + keyMaterial[keyMaterial.length - 1];
  }
}

class PoolProtocolCrypto {
  #pool;

  constructor(pool) {
    this.#pool = pool;
  }

  async perform(operation) {
    if (operation === "Ed25519Sign") {
      const settlement = await this.#pool.donateEd25519Sign(
        new Uint8Array(32).fill(0x11),
        new TextEncoder().encode("sign-this"),
      );
      if (settlement.tag !== "Signed") {
        throw new Error(`pooled Ed25519 sign returned ${settlement.tag}`);
      }
      const signature = settlement.data.signature;
      return signature[0] + signature[signature.length - 1];
    }
    if (operation === "Ed25519Verify") {
      const settlement = await this.#pool.donateEd25519Verify(
        bytesFromHex("d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"),
        new TextEncoder().encode("sign-this"),
        protocolSignature(),
      );
      if (settlement.tag !== "Valid") {
        throw new Error(`pooled Ed25519 verification returned ${settlement.tag}`);
      }
      return 1;
    }
    if (operation === "X25519") {
      const settlement = await this.#pool.donateX25519Derive(
        new Uint8Array(32).fill(0x22),
        bytesFromHex("7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14"),
      );
      if (settlement.tag !== "Derived") {
        throw new Error(`pooled X25519 derive returned ${settlement.tag}`);
      }
      const shared = settlement.data.sharedSecret;
      return shared[0] + shared[shared.length - 1];
    }
    const settlement = await this.#pool.donateHkdfSha256(protocolHkdfJob());
    if (settlement.tag !== "Derived") {
      throw new Error(`pooled HKDF-SHA256 returned ${settlement.tag}`);
    }
    const keyMaterial = settlement.data.keyMaterial;
    return keyMaterial[0] + keyMaterial[keyMaterial.length - 1];
  }
}

function protocolSignature() {
  return bytesFromHex(
    "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a" +
    "36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
  );
}

function protocolHkdfJob() {
  return {
    inputKeyMaterial: new Uint8Array(32).fill(0x42),
    salt: new Uint8Array(16).fill(0x01),
    info: new TextEncoder().encode("context"),
    outputBytes: 32,
  };
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
