# Android Shared-Instance Client Contract

Personal Hopspot runs Prns inside `PrnsService` as a foreground service and exposes the shared
instance to same-signature Android apps. A client should bind to the service, read the status bundle,
then attach to the reported loopback ports instead of starting its own Reticulum instance.

The service is exported, but guarded by `org.personal.hopspot.permission.PRNS_CLIENT` with
`signature` protection. A client app must be signed with the same certificate and declare:

```xml
<uses-permission android:name="org.personal.hopspot.permission.PRNS_CLIENT" />
```

Bind with:

```kotlin
val intent = Intent("org.personal.hopspot.action.BIND_PRNS_CLIENT")
    .setPackage("org.personal.hopspot")

context.bindService(intent, connection, Context.BIND_AUTO_CREATE)
```

The service speaks Android `Messenger` messages:

```kotlin
private const val MSG_REGISTER_CLIENT = 1
private const val MSG_UNREGISTER_CLIENT = 2
private const val MSG_ANNOUNCE = 3
private const val MSG_QUERY_STATUS = 4
private const val MSG_STATUS = 5
```

Register or query with a `replyTo` messenger. The reply is `MSG_STATUS` with this stable bundle:

```kotlin
private val connection = object : ServiceConnection {
    override fun onServiceConnected(name: ComponentName, binder: IBinder) {
        val service = Messenger(binder)
        val reply = Messenger(object : Handler(Looper.getMainLooper()) {
            override fun handleMessage(msg: Message) {
                if (msg.what != MSG_STATUS) return
                val status = msg.data
                val localPort = status.getInt("local_port")
                val rpcPort = status.getInt("rpc_port")
                val rpcKeyHex = status.getString("rpc_key_hex")
                val running = status.getBoolean("running")
                // Use 127.0.0.1:localPort for the local shared-instance bus.
                // Use 127.0.0.1:rpcPort plus rpcKeyHex for RNS 1.4.2 control RPC.
            }
        })
        service.send(Message.obtain(null, MSG_REGISTER_CLIENT).apply { replyTo = reply })
    }

    override fun onServiceDisconnected(name: ComponentName) {}
}
```

Status keys:

`state`, `running`, `foreground`, `instance_role`, `local_port`, `rpc_port`, `rpc_key_hex`,
`service_uptime_ms`, `runtime_uptime_ms`, `client_count`, `interface_count`,
`online_interface_count`, `local_client_count`, `route_count`, `link_count`,
`transported_link_count`, `rx_bytes`, `tx_bytes`, `rx_bps`, `tx_bps`, and optionally
`last_error`.

`local_port` is the HDLC-framed shared-instance data bus. `rpc_port` is the
`multiprocessing.connection`-compatible control RPC shim. RNS through `1.3.3` uses pickle payloads
on that channel; RNS `1.3.4` and later, including `1.4.2`, uses MessagePack. Prns supports the full
control payload set in both dialects and encodes each reply in the request's dialect. `rpc_key_hex`
is the current runtime key derived from the service identity and is only returned over the
signature-protected binder contract.
