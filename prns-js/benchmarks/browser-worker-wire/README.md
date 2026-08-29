# Browser worker wire benchmark

This opt-in benchmark compares ordinary structured-clone messages, Prns's inferred packed worker wire, and the explicit Prns command codecs in a real Chromium worker.

Run it from `prns-js`:

```console
npm run bench:browser-worker-wire
```

The harness allocates workloads outside timed regions, warms every path, rotates measurement order, and reports medians. It measures individual cloned messages, cloned batches, inferred packed batches, explicit Prns command codecs, realistic nested record arrays, dense numeric arrays, direct `Float64Array` transfer, and completion isolation with one delayed command. The benchmark is not part of package output or the default correctness suite.

The 2026-08-28 Linux run on a 12th Gen Intel Core i7-1260P with headless Chrome 151 found that the domain codec crossed its cloned control decisively. For 100-command grains repeated to 10,000 total commands per sample, cloned batches took 63.3 ms, inferred packing took 103.7 ms, and the Prns codec took 24.1 ms. At the maximum 4,096-command configuration, cloned batches took 177.2 ms versus 54.6 ms coded. The coded path was 1.60× faster at 10 items, 2.21× at 32, 2.63× at 100, and 3.25× at 4,096. Its 1.07× single-item loss was below the finding threshold, so production conservatively activates it at the first strong crossover of ten coalesced items and otherwise uses native clone. One hundred fast coded settlements arrived in 0.8 ms while an adjacent delayed command completed at 51.0 ms.

Dense numeric arrays exposed a separate representation boundary. A 100,000-value plain `number[]` round trip took 11.2 ms, inferred f64 packing plus plain-array rematerialization took 33.8 ms, and transferring a retained `Float64Array` took 0.3 ms. Native clone already handles dense numeric arrays well; the larger win requires preserving the typed representation instead of rebuilding a plain array at both edges. Realistic 100,000-row objects told the same generic-packing story: 561.0 ms cloned versus 939.7 ms inferred.

Accordingly, production batches through structured clone below the measured command threshold and uses the explicit command codec at or above it. Inferred packing remains available for experiments. Typed or columnar values should be an explicit contract choice when consumers can preserve that representation across the boundary.

Record the Chromium version, host CPU, command, and complete JSON result when using the measurements to change transport policy constants.
