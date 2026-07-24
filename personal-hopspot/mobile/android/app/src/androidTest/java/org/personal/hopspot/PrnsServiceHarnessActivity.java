package org.personal.hopspot;

import android.app.Activity;
import android.content.ComponentName;
import android.content.Intent;
import android.os.Build;
import android.os.Bundle;

public final class PrnsServiceHarnessActivity extends Activity {
    private static final String TARGET_PACKAGE = "org.personal.hopspot";
    private static final String ACTION_START = "org.personal.hopspot.action.START_PRNS";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        String nonce = getIntent().getStringExtra("nonce");
        if (nonce != null) {
            startService(
                    new Intent(this, PrnsServiceStatusProbeService.class)
                            .putExtra("nonce", nonce));
            finish();
            return;
        }
        ComponentName service =
                new ComponentName(TARGET_PACKAGE, TARGET_PACKAGE + ".PrnsService");
        Intent serviceIntent = new Intent(ACTION_START).setComponent(service);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            startForegroundService(serviceIntent);
        } else {
            startService(serviceIntent);
        }
        finish();
    }
}
