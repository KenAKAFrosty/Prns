# Browser worker wire benchmark

This opt-in benchmark compares ordinary structured-clone messages with Prns's inferred packed worker wire in a real Chromium worker.

Run it from `prns-js`:

```console
npm run bench:browser-worker-wire
```

The harness allocates workloads outside timed regions, warms every path, rotates measurement order, and reports medians. It measures individual cloned messages, cloned batches, inferred packed batches, realistic nested record arrays, and completion isolation with one delayed command. The benchmark is not part of package output or the default correctness suite.

The 2026-08-28 Linux run on headless Chrome 151 found that batching was the production win while generic inferred packing had not crossed its control. For 100-command grains repeated to 10,000 total commands per sample, cloned batches took 29.5 ms versus 63.4 ms for individual messages and 38.6 ms for inferred packed batches. The maximum 4,096-command configuration, run for ten rounds per sample, took 106.1 ms cloned versus 133.1 ms packed. A 100,000-row full round trip took 605.1 ms through structured clone and 1,017.9 ms through inferred packing. One hundred fast settlements arrived in 0.6 ms while an adjacent delayed command completed at 50.8 ms.

Accordingly, the production channel batches through structured clone by default. Inferred packing remains available for experiments, and explicitly registered codecs can own a packed crossing without changing its callers.

Record the Chromium version, host CPU, command, and complete JSON result when using the measurements to change transport policy constants.
