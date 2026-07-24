package org.personal.hopspot;

import android.app.Service;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.Message;
import android.os.Messenger;
import android.os.RemoteException;
import android.util.Log;

public final class PrnsServiceStatusProbeService extends Service {
    private static final String TARGET_PACKAGE = "org.personal.hopspot";
    private static final String ACTION_CLIENT = "org.personal.hopspot.action.BIND_PRNS_CLIENT";
    private static final String TAG = "PrnsStatusProbe";
    private static final int MSG_REGISTER_CLIENT = 1;
    private static final int MSG_UNREGISTER_CLIENT = 2;
    private static final int MSG_STATUS = 5;
    private static final long TIMEOUT_MILLIS = 5000;

    private final Handler timeoutHandler = new Handler(Looper.getMainLooper());
    private final Messenger reply = new Messenger(new StatusHandler());
    private Messenger remote;
    private boolean bound;
    private String nonce;
    private final Runnable timeout =
            new Runnable() {
                @Override
                public void run() {
                    Log.e(TAG, "ERROR nonce=" + nonce + " reason=timeout");
                    complete();
                }
            };
    private final ServiceConnection connection =
            new ServiceConnection() {
                @Override
                public void onServiceConnected(ComponentName name, IBinder binder) {
                    remote = new Messenger(binder);
                    Message register = Message.obtain(null, MSG_REGISTER_CLIENT);
                    register.replyTo = reply;
                    try {
                        remote.send(register);
                    } catch (RemoteException error) {
                        Log.e(TAG, "ERROR nonce=" + nonce + " reason=register");
                        complete();
                    }
                }

                @Override
                public void onServiceDisconnected(ComponentName name) {
                    remote = null;
                }
            };

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String requested = intent == null ? null : intent.getStringExtra("nonce");
        nonce = requested == null ? "none" : requested.replaceAll("[^A-Za-z0-9_-]", "");
        ComponentName service =
                new ComponentName(TARGET_PACKAGE, TARGET_PACKAGE + ".PrnsService");
        Intent client = new Intent(ACTION_CLIENT).setComponent(service);
        bound = bindService(client, connection, Context.BIND_AUTO_CREATE);
        if (!bound) {
            Log.e(TAG, "ERROR nonce=" + nonce + " reason=bind");
            complete();
            return START_NOT_STICKY;
        }
        timeoutHandler.postDelayed(timeout, TIMEOUT_MILLIS);
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        timeoutHandler.removeCallbacks(timeout);
        if (bound) {
            unbindService(connection);
            bound = false;
        }
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    private void complete() {
        stopSelf();
    }

    private final class StatusHandler extends Handler {
        StatusHandler() {
            super(Looper.getMainLooper());
        }

        @Override
        public void handleMessage(Message message) {
            if (message.what != MSG_STATUS) {
                super.handleMessage(message);
                return;
            }
            Bundle status = message.getData();
            StringBuilder line = new StringBuilder("STATUS nonce=").append(nonce);
            append(line, "state", status.getString("state", ""));
            append(line, "last_failure", status.getString("last_failure", ""));
            append(line, "persistence_active", status.getBoolean("persistence_active"));
            append(line, "route_count", status.getInt("route_count"));
            append(line, "restored_route_count", status.getInt("restored_route_count"));
            append(
                    line,
                    "restored_destination_identity_count",
                    status.getInt("restored_destination_identity_count"));
            append(line, "restored_tunnel_count", status.getInt("restored_tunnel_count"));
            append(line, "restored_ratchet_count", status.getInt("restored_ratchet_count"));
            append(line, "refused_restore_count", status.getInt("refused_restore_count"));
            append(line, "dropped_restore_count", status.getInt("dropped_restore_count"));
            append(line, "successful_flush_count", status.getLong("successful_flush_count"));
            Log.i(TAG, line.toString());
            Messenger service = remote;
            if (service != null) {
                Message unregister = Message.obtain(null, MSG_UNREGISTER_CLIENT);
                unregister.replyTo = reply;
                try {
                    service.send(unregister);
                } catch (RemoteException ignored) {
                    remote = null;
                }
            }
            complete();
        }
    }

    private static void append(StringBuilder line, String name, Object value) {
        line.append(' ').append(name).append('=').append(value);
    }
}
