/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * Parts of this file are derived from SDL 2's Android project template, which
 * has a different license. Please see vendor/SDL/LICENSE.txt for details.
 */
package org.podroid.android;

import android.content.Context;
import android.content.Intent;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.Display;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;
import android.view.WindowManager;
import android.widget.Toast;
import android.widget.TextView;
import android.widget.RelativeLayout;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;

import java.io.File;

import org.libsdl.app.SDLActivity;

/**
 * A wrapper class over SDLActivity
 */

public class MainActivity extends SDLActivity {
    // Public accessor for SDLActivity's protected static mSingleton, so
    // helper classes in this package (HostMedia) can reach the Activity.
    public static android.app.Activity getActivity() {
        return mSingleton;
    }

    /**
     * The command-line arguments for touchHLE (the app path and options),
     * set by LauncherActivity.
     */
    public static final String EXTRA_ARGS = "org.podroid.android.ARGS";
    /** The game's key in the launcher's library, for counting play time. */
    public static final String EXTRA_LIBRARY_KEY = "org.podroid.android.LIBRARY_KEY";

    /// When the game last came to the foreground, or 0 while it's not there.
    private long resumedAt;

    /// Lets the local multiplayer see other players' rooms: many devices drop
    /// broadcast packets to save power unless an app asks for them.
    private WifiManager.MulticastLock multicastLock;

    private boolean hasArgs() {
        return getIntent() != null && getIntent().getStringArrayExtra(EXTRA_ARGS) != null;
    }

    @Override
    protected String[] getArguments() {
        String[] args = getIntent() != null ? getIntent().getStringArrayExtra(EXTRA_ARGS) : null;
        return args != null ? args : new String[0];
    }

    @Override
    protected void attachBaseContext(Context base) {
        super.attachBaseContext(Languages.wrap(base));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // Use the whole screen, including the area beside a display cutout
        // (e.g. a front camera). Otherwise Android leaves that area empty in
        // landscape and the app content isn't centered on the screen.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            WindowManager.LayoutParams attrs = getWindow().getAttributes();
            attrs.layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            getWindow().setAttributes(attrs);
        }
        preferSixtyHertz();
        super.onCreate(savedInstanceState);
        if (!hasArgs()) {
            // Started without an app (e.g. from the notification after
            // touchHLE was closed): the launcher is where to pick one.
            startActivity(new Intent(this, LauncherActivity.class));
            finish();
            return;
        }
        hideSystemBars();
        installNoiseMarker();
        acquireMulticastLock();
    }

    private void acquireMulticastLock() {
        try {
            WifiManager wifi = (WifiManager) getApplicationContext()
                .getSystemService(Context.WIFI_SERVICE);
            multicastLock = wifi.createMulticastLock("touchHLE local multiplayer");
            multicastLock.setReferenceCounted(false);
            multicastLock.acquire();
        } catch (Throwable e) {
            // Not essential: rooms can still be found on many devices.
            multicastLock = null;
        }
    }

    private native void nativeMarkAudioNoise();

    private void installNoiseMarker() {
        boolean enabled = false;
        for (String arg : getArguments()) {
            enabled |= arg.startsWith("--audio-test=");
        }
        if (!enabled || mLayout == null) {
            return;
        }
        final float density = getResources().getDisplayMetrics().density;
        TextView marker = new TextView(this);
        marker.setText(R.string.mark_noise);
        marker.setTextColor(Color.WHITE);
        marker.setTextSize(12);
        marker.setTypeface(Typeface.DEFAULT_BOLD);
        marker.setPadding((int) (12 * density), (int) (7 * density),
            (int) (12 * density), (int) (7 * density));
        marker.setFocusable(false);
        marker.setSoundEffectsEnabled(false);
        marker.setHapticFeedbackEnabled(false);
        GradientDrawable background = new GradientDrawable();
        background.setColor(0xAA20252B);
        background.setCornerRadius(16 * density);
        marker.setBackground(background);
        marker.setOnClickListener(v -> {
            nativeMarkAudioNoise();
            Toast.makeText(this, R.string.noise_marked, Toast.LENGTH_LONG).show();
        });
        RelativeLayout.LayoutParams params = new RelativeLayout.LayoutParams(-2, -2);
        params.addRule(RelativeLayout.ALIGN_PARENT_TOP);
        params.addRule(RelativeLayout.CENTER_HORIZONTAL);
        params.topMargin = (int) (8 * density);
        mLayout.addView(marker, params);
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            hideSystemBars();
        }
    }

    // touchHLE's window isn't an SDL "fullscreen" window on Android (see
    // Window::new in src/window.rs for why), so SDL doesn't hide the status
    // and navigation bars. Do it here instead, the same way SDL would.
    @SuppressWarnings("deprecation")
    private void hideSystemBars() {
        try {
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);
            getWindow().getDecorView().setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                | View.SYSTEM_UI_FLAG_FULLSCREEN
                | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY);
        } catch (Throwable e) {
            // Not essential.
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (isFinishing()) {
            return;
        }
        resumedAt = SystemClock.elapsedRealtime();
        hideSystemBars();
        // Started while touchHLE is in the foreground, since Android doesn't
        // allow starting foreground services from the background.
        try {
            startService(new Intent(this, KeepAliveService.class));
        } catch (Throwable e) {
            // Not essential.
        }
    }

    // How long to wait for touchHLE to quit before ending the process anyway.
    // Apps normally finish saving within a second, but some never finish
    // quitting (e.g. Grand Theft Auto: Vice City), and waiting longer just
    // keeps the game on screen.
    private static final long QUIT_TIMEOUT_MS = 2000;

    @Override
    protected void onPause() {
        // touchHLE pauses the game while it's in the background, so only
        // foreground time is play time.
        String key = getIntent() != null ? getIntent().getStringExtra(EXTRA_LIBRARY_KEY) : null;
        if (key != null && resumedAt != 0) {
            File dir = getExternalFilesDir(null);
            Library.addPlayTime(dir != null ? dir : getFilesDir(), key,
                (SystemClock.elapsedRealtime() - resumedAt) / 1000);
        }
        resumedAt = 0;
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        // SDLActivity tells touchHLE to quit and waits for it, which gives the
        // app a chance to save (applicationWillTerminate:). The service keeps
        // the process alive meanwhile, so only stop it afterwards.
        // But it waits without a time limit: if touchHLE never finishes (e.g.
        // after the app crashes while quitting), this process would stay stuck
        // and opening a game again would just show it, so end it after a while
        // regardless.
        if (!isChangingConfigurations()) {
            Thread watchdog = new Thread(() -> {
                SystemClock.sleep(QUIT_TIMEOUT_MS);
                android.os.Process.killProcess(android.os.Process.myPid());
            });
            watchdog.setDaemon(true);
            watchdog.start();
        }
        super.onDestroy();
        if (multicastLock != null) {
            multicastLock.release();
        }
        try {
            stopService(new Intent(this, KeepAliveService.class));
        } catch (Throwable e) {
            // Not essential.
        }
        // This activity has its own process (see AndroidManifest.xml), and
        // touchHLE isn't made to be started twice in one process, so end it.
        // Android then returns to the launcher.
        if (!isChangingConfigurations()) {
            android.os.Process.killProcess(android.os.Process.myPid());
        }
    }

    // iPhone OS apps never run at more than 60 frames per second, and
    // touchHLE composites the screen at 60Hz. On phones with a 90Hz, 120Hz
    // or 144Hz display, that makes frames take an uneven number of refreshes
    // (stutter) and wastes battery, so ask for a 60Hz display mode with the
    // same resolution, if there is one.
    @SuppressWarnings("deprecation")
    private void preferSixtyHertz() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
            return;
        }
        try {
            Display display = getWindowManager().getDefaultDisplay();
            Display.Mode current = display.getMode();
            for (Display.Mode mode : display.getSupportedModes()) {
                if (mode.getPhysicalWidth() == current.getPhysicalWidth()
                    && mode.getPhysicalHeight() == current.getPhysicalHeight()
                    && Math.abs(mode.getRefreshRate() - 60.0f) < 1.0f) {
                    WindowManager.LayoutParams attrs = getWindow().getAttributes();
                    attrs.preferredDisplayModeId = mode.getModeId();
                    getWindow().setAttributes(attrs);
                    return;
                }
            }
        } catch (Throwable e) {
            // Not essential, and this runs during startup, so never let it
            // crash touchHLE.
        }
    }

    // Back exits the app and returns to the launcher (LauncherActivity).
    // Finishing this activity makes SDLActivity tell touchHLE to quit and wait
    // for it, so the app gets a chance to save (applicationWillTerminate:).
    //
    // With gesture navigation, swiping in from the side of the screen is
    // "back", which is easy to do by accident while playing. So back has to be
    // pressed twice in quick succession.
    private static final long BACK_CONFIRM_MS = 2000;
    private long lastBackPressTime = 0;

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        // SDL passes the back key (also sent for the back gesture) to the app
        // as a keyboard key, so onBackPressed() would never be called. The
        // app can't use it anyway, so handle it here before SDL gets it. A
        // mouse's back button is left to SDL, as a right click.
        if (event.getKeyCode() == KeyEvent.KEYCODE_BACK
            && (event.getSource() & InputDevice.SOURCE_MOUSE) != InputDevice.SOURCE_MOUSE) {
            if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) {
                onBackPressed();
            }
            return true;
        }
        return super.dispatchKeyEvent(event);
    }

    @Override
    public void onBackPressed() {
        long now = SystemClock.uptimeMillis();
        if (now - lastBackPressTime < BACK_CONFIRM_MS) {
            lastBackPressTime = 0;
            finish();
        } else {
            lastBackPressTime = now;
            Toast.makeText(this, R.string.back_again_to_exit, Toast.LENGTH_SHORT).show();
        }
    }

    @Override
    public void superOnBackPressed() {
        onBackPressed();
    }

    @Override
    protected String[] getLibraries() {
        return new String[]{
            "SDL2",
            "Podroid"
        };
    }
}
