# Prns Drop

Prns Drop is a text-first application example built on a real Worker-backed Prns browser node. Auto Wi-Fi supplies transport, application destinations provide persistent addresses, announce app-data enables optional nearby discovery, and shareable contact codes provide deterministic out-of-band rendezvous.

The networking and application state live in `app/`. The Solid-specific adapter only converts the framework-neutral immutable snapshot subscription into a Solid accessor. React and Qwik presentations can therefore reuse the protocol, contacts, persistence, delivery state machine, and Prns event ownership unchanged.

Run a native Prns browser rendezvous transport, then launch:

```sh
npm --prefix prns-js run drop:serve
```

Open `http://127.0.0.1:4176/`. To create two identities on one machine, open the second peer at `http://localhost:4176/`; the two hostnames are separate browser origins and therefore receive separate persistent browser identities.
