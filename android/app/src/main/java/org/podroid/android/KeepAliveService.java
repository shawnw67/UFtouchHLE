/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.podroid.android;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.os.Build;
import android.os.IBinder;

/**
 * A foreground service that runs while touchHLE is open.
 *
 * When touchHLE is in the background, the app it runs is paused, but any
 * progress that the app hasn't saved yet is only kept as long as Android
 * doesn't kill touchHLE's process. Many phones kill background apps quickly,
 * especially ones using a lot of memory. A foreground service (which must show
 * a notification) makes that much less likely. It does nothing else.
 *
 * Everything here is best-effort: if Android doesn't allow it, touchHLE simply
 * works as it would without it.
 */
public class KeepAliveService extends Service {
    private static final String CHANNEL_ID = "touchhle_running";
    private static final int NOTIFICATION_ID = 1;

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    protected void attachBaseContext(Context base) {
        super.attachBaseContext(Languages.wrap(base));
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        try {
            startForeground(NOTIFICATION_ID, buildNotification());
        } catch (Throwable e) {
            // Not allowed right now (e.g. touchHLE is already in the
            // background). Keep running as an ordinary service.
        }
        // If Android kills touchHLE anyway, there's nothing to keep alive.
        return START_NOT_STICKY;
    }

    @SuppressWarnings("deprecation")
    private Notification buildNotification() {
        Notification.Builder builder;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel channel = new NotificationChannel(
                CHANNEL_ID, getString(R.string.keep_alive_channel),
                NotificationManager.IMPORTANCE_LOW);
            channel.setDescription(getString(R.string.keep_alive_channel_description));
            channel.setShowBadge(false);
            NotificationManager manager =
                (NotificationManager) getSystemService(NOTIFICATION_SERVICE);
            manager.createNotificationChannel(channel);
            builder = new Notification.Builder(this, CHANNEL_ID);
        } else {
            builder = new Notification.Builder(this);
        }

        // Tapping the notification returns to touchHLE.
        Intent intent = new Intent(this, MainActivity.class);
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        int pendingIntentFlags = 0;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            // Required when targeting Android 12 or later.
            pendingIntentFlags |= PendingIntent.FLAG_IMMUTABLE;
        }
        PendingIntent pendingIntent =
            PendingIntent.getActivity(this, 0, intent, pendingIntentFlags);

        return builder
            .setSmallIcon(getApplicationInfo().icon)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(getString(R.string.keep_alive_running))
            .setContentIntent(pendingIntent)
            .setOngoing(true)
            .build();
    }
}
