# Browser worker wire benchmark

This opt-in benchmark compares ordinary structured-clone messages, same-turn cloned batches, explicit Prns command codecs, and direct typed-array transfer in a real Chromium worker.

Run it from `prns-js`:

```console
npm run bench:browser-worker-wire
```

The harness allocates workloads outside timed regions, warms every path, rotates measurement order, and reports medians. It measures individual cloned messages, cloned batches, explicit Prns command codecs, realistic nested record arrays, dense numeric arrays, direct `Float64Array` transfer, and completion isolation with one delayed command. The benchmark is not part of package output or the default correctness suite.

The 2026-08-29 Linux run on a 12th Gen Intel Core i7-1260P with headless Chrome 151 found that the domain codec crossed its cloned control decisively. For 100-command grains repeated to 10,000 total commands per sample, cloned batches took 50.2 ms and the Prns codec took 11.2 ms. At the maximum 4,096-command configuration, cloned batches took 172.4 ms versus 43.0 ms coded. The coded path was 1.71× faster than cloned batches at 10 items, 2.80× at 32, 4.48× at 100, and 4.01× at 4,096. It lost at one item, so production activates it at ten coalesced items and otherwise uses native clone. One hundred fast coded settlements arrived in 0.5 ms while an adjacent delayed command completed at 50.9 ms.

Dense numeric arrays expose a separate representation boundary. A 100,000-value plain `number[]` round trip took 11.9 ms, wrapping the same values in the generic cloned-batch envelope took 13.6 ms, and transferring a retained `Float64Array` took 0.2 ms. Native clone already handles dense numeric arrays well; the larger win requires preserving the typed representation instead of rebuilding a plain array at both edges. Realistic 100,000-row objects likewise showed no framing advantage: 495.4 ms directly cloned versus 501.1 ms through a cloned batch. Earlier inferred-packing experiments were slower than clone for both workloads, so that machinery was removed rather than retained as an unproven alternate path.

Accordingly, production batches through structured clone below the measured command threshold and uses the explicit command codec at or above it. There is no generic inferred-packing path. Experimental shapes stay in benchmark scratch until a measured domain codec earns an explicit contract. Typed or columnar values should be an explicit contract choice when consumers can preserve that representation across the boundary.

Record the Chromium version, host CPU, command, and complete JSON result when using the measurements to change transport policy constants.
