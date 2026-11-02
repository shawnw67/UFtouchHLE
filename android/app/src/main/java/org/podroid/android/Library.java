/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.podroid.android;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.res.Resources;

import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;

/**
 * What the launcher's game library keeps about each game: when it was last
 * played, for how long in all, whether it's a favourite and how far the
 * player got (which they set themselves: touchHLE can't tell).
 *
 * Play time is counted by the game's own process (see MainActivity), which
 * is the only writer of each game's play time file. Everything else is the
 * launcher's, in its preferences.
 */
final class Library {
    private Library() {}

    static final String DIR = "Podroid_library";
    private static final String PREFS = "library";

    static final int STATUS_NOT_STARTED = 0;
    static final int STATUS_PLAYING = 1;
    static final int STATUS_COMPLETED = 2;
    static final int STATUS_SHELVED = 3;

    /** A game's identity in the library: its bundle ID, or its file. */
    static String key(String bundleId, String path) {
        String id = bundleId != null && !bundleId.isEmpty()
            ? bundleId : "path-" + new File(path).getName();
        return id.replaceAll("[^A-Za-z0-9._-]", "_");
    }

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    static long lastPlayed(Context context, String key) {
        return prefs(context).getLong("lastPlayed:" + key, 0);
    }

    /** Called when a game is started. A game not started before is now being played. */
    static void started(Context context, String key) {
        SharedPreferences.Editor editor = prefs(context).edit()
            .putLong("lastPlayed:" + key, System.currentTimeMillis());
        if (status(context, key) == STATUS_NOT_STARTED) {
            editor.putInt("status:" + key, STATUS_PLAYING);
        }
        editor.apply();
    }

    static boolean favorite(Context context, String key) {
        return prefs(context).getBoolean("favorite:" + key, false);
    }

    static void setFavorite(Context context, String key, boolean favorite) {
        prefs(context).edit().putBoolean("favorite:" + key, favorite).apply();
    }

    static int status(Context context, String key) {
        return prefs(context).getInt("status:" + key, STATUS_NOT_STARTED);
    }

    static void setStatus(Context context, String key, int status) {
        prefs(context).edit().putInt("status:" + key, status).apply();
    }

    static String sortMode(Context context) {
        return prefs(context).getString("sort", "recent");
    }

    static void setSortMode(Context context, String mode) {
        prefs(context).edit().putString("sort", mode).apply();
    }

    static String filter(Context context) {
        return prefs(context).getString("filter", "all");
    }

    static void setFilter(Context context, String filter) {
        prefs(context).edit().putString("filter", filter).apply();
    }

    // ---- Play time ----

    private static File playTimeFile(File baseDir, String key) {
        return new File(new File(baseDir, DIR), key + ".playtime");
    }

    /** Total play time, in seconds. */
    static long playTime(File baseDir, String key) {
        File file = playTimeFile(baseDir, key);
        if (!file.isFile() || file.length() > 64) {
            return 0;
        }
        try (java.io.FileInputStream in = new java.io.FileInputStream(file)) {
            byte[] bytes = new byte[(int) file.length()];
            int read = 0;
            while (read < bytes.length) {
                int n = in.read(bytes, read, bytes.length - read);
                if (n < 0) {
                    break;
                }
                read += n;
            }
            return Long.parseLong(new String(bytes, 0, read, StandardCharsets.UTF_8).trim());
        } catch (Exception e) {
            return 0;
        }
    }

    /** Adds to a game's play time. Only the game's own process calls this. */
    static void addPlayTime(File baseDir, String key, long seconds) {
        if (seconds <= 0) {
            return;
        }
        File file = playTimeFile(baseDir, key);
        file.getParentFile().mkdirs();
        long total = playTime(baseDir, key) + seconds;
        File tmp = new File(file.getPath() + ".tmp");
        try (FileOutputStream out = new FileOutputStream(tmp)) {
            out.write(Long.toString(total).getBytes(StandardCharsets.UTF_8));
        } catch (Exception e) {
            return;
        }
        tmp.renameTo(file);
    }

    // ---- Text ----

    /** How long ago, e.g. "3 days ago". The UI's language, not the system's. */
    static String ago(Context context, long time) {
        Resources res = context.getResources();
        long minutes = Math.max(0, (System.currentTimeMillis() - time) / 60000);
        if (minutes < 1) {
            return res.getString(R.string.time_just_now);
        }
        if (minutes < 60) {
            int n = (int) minutes;
            return res.getQuantityString(R.plurals.time_minutes_ago, n, n);
        }
        if (minutes < 24 * 60) {
            int n = (int) (minutes / 60);
            return res.getQuantityString(R.plurals.time_hours_ago, n, n);
        }
        int n = (int) Math.min(minutes / (24 * 60), Integer.MAX_VALUE);
        return res.getQuantityString(R.plurals.time_days_ago, n, n);
    }

    /** A play time, e.g. "12 h" or "45 min". */
    static String duration(Context context, long seconds) {
        Resources res = context.getResources();
        long minutes = seconds / 60;
        if (minutes < 60) {
            return res.getString(R.string.duration_minutes, Math.max(1, minutes));
        }
        long hours = minutes / 60;
        return hours < 10 && minutes % 60 != 0
            ? res.getString(R.string.duration_hours_minutes, hours, minutes % 60)
            : res.getString(R.string.duration_hours, hours);
    }
}
