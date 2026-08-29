# Solid Auto Wi-Fi

This example runs a real Worker-backed Prns browser node, starts Auto Wi-Fi, and renders its typed controller status through Solid signals. It probes the same local rendezvous endpoints as the browser playground, including `ws://localhost:42721/prns` and `ws://prns.local:42721/prns`.

Run a native Prns node with its browser rendezvous transport enabled, then launch:

```sh
npm --prefix prns-js run solid-auto-wifi:serve
```

Open `http://127.0.0.1:4174/`. The example does not start a fixture or occupy the rendezvous port.
