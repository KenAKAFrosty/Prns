# Browser projection smoke

This opt-in Chromium smoke measures the two scheduling boundaries behind the browser framework adapters: local projection notification and the latest-state Worker projection channel.

Run it from `prns-js`:

```console
npm run bench:browser-projections
```

The workload publishes 10,000 lifecycle transitions in one task. Correct coalescing produces one subscriber notification, one cloned frame, one decoded latest-state update, and a final `Running` lifecycle. The timings are descriptive smoke evidence, not a comparative performance verdict; use the rotated and warmed browser Worker wire benchmark for transport-policy changes.

The 2026-08-29 Linux run on a 12th Gen Intel Core i7-1260P with Chromium 151.0.7922.108 issued the 10,000 local publications in 7.9 ms and settled their single notification in 8.1 ms. The Worker projection sender accepted the same 10,000 publications in 2.0 ms and delivered one cloned latest-state frame in 3.2 ms. These are one-run smoke timings; the structural counts are the assertion.
