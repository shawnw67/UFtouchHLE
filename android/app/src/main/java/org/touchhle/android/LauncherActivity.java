/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.Dialog;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.res.ColorStateList;
import android.content.res.Configuration;
import android.database.Cursor;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.ColorFilter;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.PixelFormat;
import android.graphics.Rect;
import android.graphics.Typeface;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.RippleDrawable;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.text.TextUtils;
import android.util.TypedValue;
import android.view.DisplayCutout;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;
import android.widget.BaseAdapter;
import android.widget.FrameLayout;
import android.widget.GridView;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Switch;
import android.widget.TextView;
import android.widget.Toast;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.RandomAccessFile;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.text.SimpleDateFormat;
import java.util.Date;
import java.util.Locale;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;
import java.util.zip.ZipOutputStream;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.json.JSONArray;
import org.json.JSONObject;

/**
 * touchHLE's launcher on Android: lists the apps, imports new ones, and lets
 * the quick options be changed, then starts MainActivity (touchHLE proper,
 * which runs in its own process) with the chosen app.
 *
 * This replaces the app picker (src/environment/app_picker.rs) on Android.
 * That one is an emulated iPhone OS app, so it's drawn at 320×480 and scaled
 * up, which makes it blurry and leaves black bars on modern screens. This
 * activity is drawn at the screen's real resolution and fills any shape of
 * screen, in either orientation.
 */
public class LauncherActivity extends Activity {
    static {
        System.loadLibrary("SDL2");
        System.loadLibrary("touchHLE");
    }

    /** See src/android_launcher.rs. */
    private static native byte[] nativeScanApps(String appsDir, String iconsDir, String languages);
    private static native byte[] nativeCopyrightText(android.content.res.AssetManager assets);
    private static native byte[] nativeVersion();
    private static native byte[] nativeImportPspSave(byte[] file, byte[] sfo, byte[] current);

    private static final String APPS_DIR = "touchHLE_apps";
    private static final String LOG_FILE = "touchHLE_log.txt";
    private static final String[] WALLPAPER_FILES = {
        "touchHLE_wallpaper.png", "touchHLE_wallpaper.jpg", "touchHLE_wallpaper.jpeg",
    };
    private static final int REQUEST_IMPORT = 1;
    private static final int REQUEST_IMPORT_SAVE = 2;
    private static final int REQUEST_EXPORT_SAVE = 3;
    private static final int REQUEST_IMPORT_PSP_SAVE_FOLDER = 4;
    private static final int REQUEST_IMPORT_PSP_SAVE_FILES = 5;
    private static final String MHP2G_BUNDLE_ID = "jp.co.capcom.mhp2gjp";
    /// Monster Hunter Freedom Unite, P2G's Western release (same engine).
    private static final String MHFU_BUNDLE_ID = "jp.co.capcom.mhp2gwd";
    /// The folders (in PSP/SAVEDATA) of the PSP saves that can be imported
    /// into either game: MHP2G, and Freedom Unite in North America and Europe.
    private static final String[] PSP_SAVE_FOLDERS = {"ULJM05500", "ULUS10391", "ULES01213"};
    /// Where touchHLE keeps each app's data (see src/paths.rs), in a folder
    /// named after the app's bundle identifier.
    private static final String SANDBOX_DIR = "touchHLE_sandbox";
    /// Saves are backed up here before importing others over them.
    private static final String SAVE_BACKUPS_DIR = "touchHLE_save_backups";
    /// Exported saves go here if the system can't ask where to save them.
    private static final String SAVE_EXPORTS_DIR = "touchHLE_save_exports";
    /// Added to exported saves, to tell which app they're for.
    private static final String SAVE_MARKER = "touchHLE_save.txt";

    /// The newest release of this fork, checked for updates.
    private static final String LATEST_RELEASE_URL =
        "https://api.github.com/repos/shawnw67/UFtouchHLE/releases/latest";
    private static final String PREFS = "launcher";
    private static final String PREF_CHECK_UPDATES = "checkUpdates";
    private static final String PREF_ONLINE_POSTERS = "onlinePosters";
    private static final String PREF_IGNORED_VERSION = "ignoredUpdateVersion";
    private static final String PREF_P2G_WIDE = "p2gExpandedView";
    private static final String PREF_MHFU_WIDE = "mhfuExpandedView";

    /// The games P2G's expanded view (--p2g-wide-screen) supports, and where
    /// each one's choice is kept.
    private static String expandedViewPref(String bundleId) {
        if (MHP2G_BUNDLE_ID.equals(bundleId)) {
            return PREF_P2G_WIDE;
        }
        if (MHFU_BUNDLE_ID.equals(bundleId)) {
            return PREF_MHFU_WIDE;
        }
        return null;
    }
    private static final String PREF_DEVICE_PROFILE = "deviceProfile";
    private static final String PREF_APP_DEVICE_PROFILE = "appDeviceProfile:";
    /// Models with the same screen are offered once: the one that's most
    /// widely recognised by games (and that automatic selection uses).
    private static final String[] DEVICE_PROFILES = {
        "auto", "iPhone2,1", "iPhone4,1", "iPhone5,1",
        "iPhone7,2", "iPhone7,1", "iPad2,1", "iPad3,1",
    };

    /// Choices that were merged with another model of the same screen.
    private static String mergedDeviceProfile(String value) {
        if ("iPhone3,1".equals(value)) {
            return "iPhone4,1";
        }
        if ("iPhone6,1".equals(value)) {
            return "iPhone5,1";
        }
        return value;
    }

    private static final int TEXT = Color.WHITE;
    private static final int TEXT_DIM = 0xB3FFFFFF;
    private static final int ACCENT = 0xFF3D8BFF;
    private static final int WARNING = 0xFFFFC766;

    private static class App {
        String path;
        String name;
        Bitmap icon;
        /// Box art, or one drawn from the icon (see [Posters]).
        Bitmap poster;
        /// See [Library].
        String key;
        long lastPlayed;
        long playTime;
        boolean favorite;
        int status;
        /// Empty if unknown.
        String bundleId;
    }

    /// The app whose saves are being imported or exported (kept across
    /// activity recreation, since the system file picker is in between).
    private String saveBundleId;
    private String saveAppName;

    /// Every game, and the ones shown with the library's filter and order.
    private final List<App> allApps = new ArrayList<>();
    private final List<App> apps = new ArrayList<>();
    private FrameLayout continueSlot;
    private LinearLayout filterBar;
    private boolean loading = true;
    private QuickOptions quickOptions;
    private Bitmap wallpaper;

    private AppAdapter adapter;
    private TextView statusText;
    /// Holds the "new version" banner, filled in once the check finishes.
    private FrameLayout updateSlot;
    /// The newer release found by [checkForUpdate], if any.
    private UpdateInfo update;

    private static class UpdateInfo {
        String version;
        String notes;
        /// The Android APK, or the release page if there's none.
        String downloadUrl;
    }
    private View importBigButton;

    // ---- Lifecycle ----

    @Override
    protected void attachBaseContext(android.content.Context base) {
        super.attachBaseContext(Languages.wrap(base));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Intent intent = getIntent();
        if (!isTaskRoot() && intent != null && intent.hasCategory(Intent.CATEGORY_LAUNCHER)
            && Intent.ACTION_MAIN.equals(intent.getAction())) {
            // Opened from the home screen while an app is running on top of
            // the launcher: just go back to it.
            finish();
            return;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            WindowManager.LayoutParams attrs = getWindow().getAttributes();
            attrs.layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            getWindow().setAttributes(attrs);
        }
        getAppsDir().mkdirs();
        if (savedInstanceState != null) {
            saveBundleId = savedInstanceState.getString("saveBundleId");
            saveAppName = savedInstanceState.getString("saveAppName");
        }
        quickOptions = QuickOptions.load(getBaseDir());
        if (savedInstanceState == null && playFromShortcut(intent)) {
            // Leaving the game goes back to the home screen it came from.
            finish();
            return;
        }
        wallpaper = loadWallpaper();
        buildUi();
        rescan();
        checkForUpdate();
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        super.onSaveInstanceState(outState);
        outState.putString("saveBundleId", saveBundleId);
        outState.putString("saveAppName", saveAppName);
    }

    @Override
    protected void onResume() {
        super.onResume();
        hideSystemBars();
        // touchHLE may have just exited (e.g. after a crash, whose log should
        // be pointed out) or files may have been changed elsewhere.
        if (!loading) {
            buildUi();
            rescan();
        }
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            hideSystemBars();
        }
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        // The layout differs between portrait and landscape.
        buildUi();
    }

    @SuppressWarnings("deprecation")
    private void hideSystemBars() {
        try {
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

    // ---- Files ----

    private File getBaseDir() {
        // Same as SDL_AndroidGetExternalStoragePath(), which touchHLE uses.
        File dir = getExternalFilesDir(null);
        return dir != null ? dir : getFilesDir();
    }

    private File getAppsDir() {
        return new File(getBaseDir(), APPS_DIR);
    }

    private void rescan() {
        loading = true;
        updateStatus();
        final File iconsDir = new File(getCacheDir(), "launcher_icons");
        new Thread(() -> {
            File[] old = iconsDir.listFiles();
            if (old != null) {
                for (File file : old) {
                    file.delete();
                }
            }
            String result = new String(
                nativeScanApps(getAppsDir().getPath(), iconsDir.getPath(), Languages.preferred(this)),
                StandardCharsets.UTF_8);
            final List<App> found = new ArrayList<>();
            for (String line : result.split("\n")) {
                String[] fields = line.split("\t", -1);
                if (fields.length < 5) {
                    continue;
                }
                App app = new App();
                app.path = fields[0];
                app.name = fields[1];
                app.icon = loadIcon(fields[2], fields[3], fields[4]);
                app.bundleId = fields.length > 5 ? fields[5] : "";
                File postersDir = getPostersDir();
                app.poster = Posters.cached(postersDir, app.bundleId, app.name);
                if (app.poster == null) {
                    app.poster = Posters.fromArtwork(app.path);
                }
                if (app.poster == null) {
                    app.poster = Posters.generate(app.icon);
                }
                loadLibraryInfo(app);
                found.add(app);
            }
            runOnUiThread(() -> {
                allApps.clear();
                allApps.addAll(found);
                loading = false;
                refreshLibrary();
                fetchPosters(found);
            });
        }).start();
    }

    private File getPostersDir() {
        return new File(getBaseDir(), Posters.DIR);
    }

    /// Looks for box art online for the games that don't have any yet, and
    /// shows each as it arrives.
    private void fetchPosters(final List<App> found) {
        if (!getSharedPreferences(PREFS, MODE_PRIVATE).getBoolean(PREF_ONLINE_POSTERS, true)) {
            return;
        }
        final File dir = getPostersDir();
        final String userAgent = "UFtouchHLE/"
            + new String(nativeVersion(), StandardCharsets.UTF_8).trim()
            + " (https://github.com/shawnw67/UFtouchHLE)";
        new Thread(() -> {
            for (final App app : found) {
                if (!Posters.shouldFetch(dir, app.bundleId, app.name)) {
                    continue;
                }
                final Bitmap poster = Posters.fetch(dir, app.bundleId, app.name, userAgent);
                if (poster == null) {
                    continue;
                }
                runOnUiThread(() -> {
                    app.poster = poster;
                    if (adapter != null) {
                        adapter.notifyDataSetChanged();
                    }
                });
            }
        }).start();
    }

    private static Bitmap loadIcon(String path, String width, String height) {
        if (path.isEmpty()) {
            return null;
        }
        try {
            int w = Integer.parseInt(width);
            int h = Integer.parseInt(height);
            byte[] pixels = readFile(new File(path), Integer.MAX_VALUE);
            if (pixels == null || pixels.length != w * h * 4) {
                return null;
            }
            Bitmap bitmap = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888);
            bitmap.copyPixelsFromBuffer(ByteBuffer.wrap(pixels));
            return bitmap;
        } catch (Exception e) {
            return null;
        }
    }

    /** Reads (at most the last {@code maxBytes} of) a file. */
    private static byte[] readFile(File file, int maxBytes) {
        try (RandomAccessFile f = new RandomAccessFile(file, "r")) {
            long length = f.length();
            int count = (int) Math.min(length, maxBytes);
            byte[] bytes = new byte[count];
            f.seek(length - count);
            f.readFully(bytes);
            return bytes;
        } catch (Exception e) {
            return null;
        }
    }

    private Bitmap loadWallpaper() {
        for (String name : WALLPAPER_FILES) {
            File file = new File(getBaseDir(), name);
            if (!file.isFile()) {
                continue;
            }
            try {
                BitmapFactory.Options options = new BitmapFactory.Options();
                options.inJustDecodeBounds = true;
                BitmapFactory.decodeFile(file.getPath(), options);
                int screen = Math.max(
                    getResources().getDisplayMetrics().widthPixels,
                    getResources().getDisplayMetrics().heightPixels);
                int sample = 1;
                while (Math.max(options.outWidth, options.outHeight) / (sample * 2) >= screen) {
                    sample *= 2;
                }
                options = new BitmapFactory.Options();
                options.inSampleSize = sample;
                return BitmapFactory.decodeFile(file.getPath(), options);
            } catch (Throwable e) {
                return null;
            }
        }
        return null;
    }

    /** Whether the last run of touchHLE crashed, judging by its log. */
    private boolean lastRunCrashed() {
        byte[] tail = readFile(new File(getBaseDir(), LOG_FILE), 64 * 1024);
        if (tail == null) {
            return false;
        }
        String log = new String(tail, StandardCharsets.UTF_8);
        // Written by the panic hook (see src/lib.rs and src/environment.rs).
        return log.contains("Panic at ") || log.contains("Panic: ")
            || log.contains("Register state immediately after panic");
    }

    // ---- UI ----

    private int dp(float value) {
        return Math.round(TypedValue.applyDimension(
            TypedValue.COMPLEX_UNIT_DIP, value, getResources().getDisplayMetrics()));
    }

    private boolean isLandscape() {
        return getResources().getConfiguration().orientation
            == Configuration.ORIENTATION_LANDSCAPE;
    }

    private Drawable buttonBackground(int color, float radius) {
        GradientDrawable shape = new GradientDrawable();
        shape.setColor(color);
        shape.setCornerRadius(radius);
        return new RippleDrawable(ColorStateList.valueOf(0x40FFFFFF), shape, null);
    }

    private TextView makeButton(String title, boolean primary, View.OnClickListener onClick) {
        TextView button = new TextView(this);
        button.setText(title);
        button.setTextColor(TEXT);
        button.setTextSize(TypedValue.COMPLEX_UNIT_SP, 15);
        button.setTypeface(Typeface.DEFAULT_BOLD);
        button.setGravity(Gravity.CENTER);
        button.setSingleLine(true);
        button.setPadding(dp(18), 0, dp(18), 0);
        button.setMinHeight(dp(44));
        button.setBackground(buttonBackground(primary ? ACCENT : 0x2EFFFFFF, dp(22)));
        button.setOnClickListener(onClick);
        return button;
    }

    private TextView makeText(String text, float sizeSp, int color, boolean bold) {
        TextView view = new TextView(this);
        view.setText(text);
        view.setTextSize(TypedValue.COMPLEX_UNIT_SP, sizeSp);
        view.setTextColor(color);
        if (bold) {
            view.setTypeface(Typeface.DEFAULT_BOLD);
        }
        return view;
    }

    private void buildUi() {
        FrameLayout root = new FrameLayout(this);
        if (wallpaper != null) {
            ImageView background = new ImageView(this);
            background.setImageBitmap(wallpaper);
            background.setScaleType(ImageView.ScaleType.CENTER_CROP);
            root.addView(background, new FrameLayout.LayoutParams(-1, -1));
            View shade = new View(this);
            shade.setBackgroundColor(0x66000000);
            root.addView(shade, new FrameLayout.LayoutParams(-1, -1));
        } else {
            root.setBackground(new GradientDrawable(
                GradientDrawable.Orientation.TL_BR,
                new int[]{0xFF27306B, 0xFF1A3F6E, 0xFF0F6A85}));
        }

        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        root.addView(content, new FrameLayout.LayoutParams(-1, -1));
        final int pad = dp(isLandscape() ? 20 : 16);
        content.setPadding(pad, pad, pad, pad);
        // Keep clear of display cutouts (e.g. the front camera).
        content.setOnApplyWindowInsetsListener((view, insets) -> {
            int left = 0, top = 0, right = 0, bottom = 0;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                DisplayCutout cutout = insets.getDisplayCutout();
                if (cutout != null) {
                    left = cutout.getSafeInsetLeft();
                    top = cutout.getSafeInsetTop();
                    right = cutout.getSafeInsetRight();
                    bottom = cutout.getSafeInsetBottom();
                }
            }
            view.setPadding(pad + left, pad + top, pad + right, pad + bottom);
            return insets;
        });

        // Header: icon, name, hint (and the actions, in landscape)
        LinearLayout header = new LinearLayout(this);
        header.setOrientation(LinearLayout.HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);
        ImageView logo = new ImageView(this);
        logo.setImageResource(BuildConfig.APP_ICON);
        // Rounded like the app icons (see Bundle::load_icon()).
        GradientDrawable logoShape = new GradientDrawable();
        logoShape.setCornerRadius(dp(48) * 10f / 57f);
        logo.setBackground(logoShape);
        logo.setClipToOutline(true);
        header.addView(logo, new LinearLayout.LayoutParams(dp(48), dp(48)));
        LinearLayout titles = new LinearLayout(this);
        titles.setOrientation(LinearLayout.VERTICAL);
        titles.setPadding(dp(12), 0, dp(12), 0);
        titles.addView(makeText(BuildConfig.APP_NAME, 22, TEXT, true));
        titles.addView(makeText(getString(R.string.launcher_hint), 13, TEXT_DIM, false));
        header.addView(titles, new LinearLayout.LayoutParams(0, -2, 1));
        content.addView(header, new LinearLayout.LayoutParams(-1, -2));

        LinearLayout actions = new LinearLayout(this);
        actions.setOrientation(LinearLayout.HORIZONTAL);
        actions.setGravity(Gravity.CENTER_VERTICAL);
        int buttonHeight = isLandscape() ? dp(44) : dp(46);
        actions.addView(makeButton(getString(R.string.import_games), true, v -> startImport()), isLandscape()
            ? new LinearLayout.LayoutParams(-2, buttonHeight)
            : new LinearLayout.LayoutParams(0, buttonHeight, 1));
        ImageView settings = new ImageView(this);
        settings.setImageDrawable(new GearDrawable(TEXT));
        settings.setScaleType(ImageView.ScaleType.FIT_CENTER);
        int iconPadding = (buttonHeight - dp(24)) / 2;
        settings.setPadding(iconPadding, iconPadding, iconPadding, iconPadding);
        settings.setBackground(buttonBackground(0x2EFFFFFF, buttonHeight / 2f));
        settings.setContentDescription(getString(R.string.settings));
        settings.setOnClickListener(v -> showQuickOptions());
        LinearLayout.LayoutParams settingsParams =
            new LinearLayout.LayoutParams(buttonHeight, buttonHeight);
        settingsParams.leftMargin = dp(10);
        actions.addView(settings, settingsParams);
        if (isLandscape()) {
            header.addView(actions);
        }

        if (lastRunCrashed()) {
            TextView banner = makeText(
                getString(R.string.last_run_crashed), 14, 0xFF3A2A00, true);
            banner.setGravity(Gravity.CENTER);
            banner.setPadding(dp(14), dp(10), dp(14), dp(10));
            banner.setBackground(buttonBackground(WARNING, dp(12)));
            banner.setOnClickListener(v -> showLog());
            LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
            params.topMargin = dp(14);
            content.addView(banner, params);
        }
        updateSlot = new FrameLayout(this);
        content.addView(updateSlot, new LinearLayout.LayoutParams(-1, -2));
        showUpdateBanner();

        continueSlot = new FrameLayout(this);
        content.addView(continueSlot, new LinearLayout.LayoutParams(-1, -2));
        android.widget.HorizontalScrollView filterScroll =
            new android.widget.HorizontalScrollView(this);
        filterScroll.setHorizontalScrollBarEnabled(false);
        filterBar = new LinearLayout(this);
        filterBar.setOrientation(LinearLayout.HORIZONTAL);
        filterBar.setGravity(Gravity.CENTER_VERTICAL);
        filterScroll.addView(filterBar);
        LinearLayout.LayoutParams filterParams = new LinearLayout.LayoutParams(-1, -2);
        filterParams.topMargin = dp(12);
        content.addView(filterScroll, filterParams);

        View divider = new View(this);
        divider.setBackgroundColor(0x33FFFFFF);
        LinearLayout.LayoutParams dividerParams = new LinearLayout.LayoutParams(-1, dp(1));
        dividerParams.topMargin = dp(14);
        content.addView(divider, dividerParams);

        // The apps, or a message where there are none
        FrameLayout middle = new FrameLayout(this);
        content.addView(middle, new LinearLayout.LayoutParams(-1, 0, 1));

        GridView grid = new GridView(this);
        grid.setNumColumns(GridView.AUTO_FIT);
        // Tablets get bigger posters, so they don't look lost on the screen.
        boolean tablet = getResources().getConfiguration().smallestScreenWidthDp >= 600;
        grid.setColumnWidth(dp(tablet ? 190 : 108));
        grid.setStretchMode(GridView.STRETCH_COLUMN_WIDTH);
        grid.setHorizontalSpacing(dp(12));
        grid.setVerticalSpacing(dp(16));
        grid.setPadding(0, dp(14), 0, dp(14));
        grid.setClipToPadding(false);
        grid.setSelector(new ColorDrawable(Color.TRANSPARENT));
        grid.setVerticalScrollBarEnabled(false);
        adapter = new AppAdapter();
        grid.setAdapter(adapter);
        grid.setOnItemClickListener((parent, view, position, id) -> launch(apps.get(position)));
        grid.setOnItemLongClickListener((parent, view, position, id) -> {
            showDetails(apps.get(position));
            return true;
        });
        middle.addView(grid, new FrameLayout.LayoutParams(-1, -1));

        LinearLayout empty = new LinearLayout(this);
        empty.setOrientation(LinearLayout.VERTICAL);
        empty.setGravity(Gravity.CENTER);
        statusText = makeText("", 16, TEXT_DIM, false);
        statusText.setGravity(Gravity.CENTER);
        statusText.setLineSpacing(0, 1.2f);
        empty.addView(statusText);
        importBigButton = makeButton(getString(R.string.import_games), true, v -> startImport());
        LinearLayout.LayoutParams bigParams = new LinearLayout.LayoutParams(dp(200), dp(50));
        bigParams.topMargin = dp(20);
        empty.addView(importBigButton, bigParams);
        middle.addView(empty, new FrameLayout.LayoutParams(-1, -1));

        if (!isLandscape()) {
            content.addView(actions, new LinearLayout.LayoutParams(-1, -2));
        }

        TextView version = makeText(
            new String(nativeVersion(), StandardCharsets.UTF_8), 11, 0x80FFFFFF, false);
        version.setGravity(Gravity.CENTER);
        version.setPadding(0, dp(8), 0, 0);
        content.addView(version, new LinearLayout.LayoutParams(-1, -2));

        setContentView(root);
        refreshLibrary();
    }

    private void updateStatus() {
        if (statusText == null) {
            return;
        }
        boolean showMessage = loading || apps.isEmpty();
        ((View) statusText.getParent()).setVisibility(showMessage ? View.VISIBLE : View.GONE);
        if (loading) {
            statusText.setText(R.string.loading_games);
            importBigButton.setVisibility(View.GONE);
        } else if (allApps.isEmpty()) {
            statusText.setText(R.string.no_games);
            importBigButton.setVisibility(View.VISIBLE);
        } else {
            statusText.setText(R.string.filter_empty);
            importBigButton.setVisibility(View.GONE);
        }
    }

    // ---- Library ----

    private static final String[] FILTERS = {"all", "playing", "completed", "shelved", "favorites"};
    private static final String[] SORTS = {"recent", "name", "playTime"};

    /// Reads what the library knows about a game. Off the UI thread is fine.
    private void loadLibraryInfo(App app) {
        app.key = Library.key(app.bundleId, app.path);
        app.lastPlayed = Library.lastPlayed(this, app.key);
        app.playTime = Library.playTime(getBaseDir(), app.key);
        app.favorite = Library.favorite(this, app.key);
        app.status = Library.status(this, app.key);
    }

    private static boolean matchesFilter(App app, String filter) {
        switch (filter) {
            case "playing":
                return app.status == Library.STATUS_PLAYING;
            case "completed":
                return app.status == Library.STATUS_COMPLETED;
            case "shelved":
                return app.status == Library.STATUS_SHELVED;
            case "favorites":
                return app.favorite;
            default:
                return true;
        }
    }

    /// Shows the games with the chosen filter and order: favourites first,
    /// then by when they were last played (games not played yet last), name
    /// or play time.
    private void refreshLibrary() {
        String filter = Library.filter(this);
        String sort = Library.sortMode(this);
        final java.text.Collator collator = java.text.Collator.getInstance();
        List<App> shown = new ArrayList<>();
        for (App app : allApps) {
            if (matchesFilter(app, filter)) {
                shown.add(app);
            }
        }
        java.util.Collections.sort(shown, (a, b) -> {
            if (a.favorite != b.favorite) {
                return a.favorite ? -1 : 1;
            }
            int order = 0;
            if ("recent".equals(sort)) {
                order = Long.compare(b.lastPlayed, a.lastPlayed);
            } else if ("playTime".equals(sort)) {
                order = Long.compare(b.playTime, a.playTime);
            }
            return order != 0 ? order : collator.compare(a.name, b.name);
        });
        apps.clear();
        apps.addAll(shown);
        if (adapter != null) {
            adapter.notifyDataSetChanged();
        }
        updateStatus();
        showFilterBar();
        showContinueBanner();
    }

    private String[] filterTitles() {
        return new String[]{getString(R.string.filter_all), getString(R.string.filter_playing),
            getString(R.string.filter_completed), getString(R.string.filter_shelved),
            getString(R.string.filter_favorites)};
    }

    private String[] sortTitles() {
        return new String[]{getString(R.string.sort_recent), getString(R.string.sort_name),
            getString(R.string.sort_play_time)};
    }

    private void showFilterBar() {
        if (filterBar == null) {
            return;
        }
        filterBar.removeAllViews();
        String filter = Library.filter(this);
        String[] titles = filterTitles();
        for (int i = 0; i < FILTERS.length; i++) {
            final String value = FILTERS[i];
            boolean on = value.equals(filter);
            TextView chip = makeText(titles[i], 14, TEXT, on);
            chip.setGravity(Gravity.CENTER);
            chip.setPadding(dp(14), 0, dp(14), 0);
            chip.setBackground(buttonBackground(on ? ACCENT : 0x2EFFFFFF, dp(17)));
            chip.setOnClickListener(v -> {
                Library.setFilter(this, value);
                refreshLibrary();
            });
            LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-2, dp(34));
            params.rightMargin = dp(8);
            filterBar.addView(chip, params);
        }
        int sort = java.util.Arrays.asList(SORTS).indexOf(Library.sortMode(this));
        TextView sortChip = makeText(
            getString(R.string.sort_label, sortTitles()[Math.max(sort, 0)]), 14, TEXT, false);
        sortChip.setGravity(Gravity.CENTER);
        sortChip.setPadding(dp(14), 0, dp(14), 0);
        sortChip.setBackground(buttonBackground(0x2EFFFFFF, dp(17)));
        sortChip.setOnClickListener(v -> new AlertDialog.Builder(
                this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle(R.string.sort_title)
            .setSingleChoiceItems(sortTitles(), Math.max(sort, 0), (dialog, which) -> {
                Library.setSortMode(this, SORTS[which]);
                dialog.dismiss();
                refreshLibrary();
            })
            .show());
        filterBar.addView(sortChip, new LinearLayout.LayoutParams(-2, dp(34)));
    }

    /// "3 days ago · 12 h", or empty for a game not played yet.
    private String playedText(App app) {
        if (app.lastPlayed == 0) {
            return "";
        }
        String text = Library.ago(this, app.lastPlayed);
        return app.playTime > 0 ? text + " · " + Library.duration(this, app.playTime) : text;
    }

    /// A banner for going back to the game played last. Phones in landscape
    /// don't have the room for it.
    private void showContinueBanner() {
        if (continueSlot == null) {
            return;
        }
        continueSlot.removeAllViews();
        boolean phone = getResources().getConfiguration().smallestScreenWidthDp < 600;
        if (phone && isLandscape()) {
            return;
        }
        App last = null;
        for (App app : allApps) {
            if (app.lastPlayed > 0 && (last == null || app.lastPlayed > last.lastPlayed)) {
                last = app;
            }
        }
        if (last == null) {
            return;
        }
        final App app = last;
        LinearLayout banner = new LinearLayout(this);
        banner.setOrientation(LinearLayout.HORIZONTAL);
        banner.setGravity(Gravity.CENTER_VERTICAL);
        banner.setPadding(dp(10), dp(10), dp(14), dp(10));
        banner.setBackground(buttonBackground(0x26FFFFFF, dp(14)));
        banner.setOnClickListener(v -> launch(app));
        ImageView poster = new ImageView(this);
        poster.setImageBitmap(app.poster);
        poster.setScaleType(ImageView.ScaleType.CENTER_CROP);
        GradientDrawable shape = new GradientDrawable();
        shape.setCornerRadius(dp(8));
        poster.setBackground(shape);
        poster.setClipToOutline(true);
        banner.addView(poster, new LinearLayout.LayoutParams(dp(54), dp(72)));
        LinearLayout texts = new LinearLayout(this);
        texts.setOrientation(LinearLayout.VERTICAL);
        texts.setPadding(dp(14), 0, dp(10), 0);
        texts.addView(makeText(getString(R.string.continue_playing), 12, TEXT_DIM, false));
        TextView name = makeText(app.name, 17, TEXT, true);
        name.setSingleLine(true);
        name.setEllipsize(TextUtils.TruncateAt.END);
        texts.addView(name);
        texts.addView(makeText(playedText(app), 13, TEXT_DIM, false));
        banner.addView(texts, new LinearLayout.LayoutParams(0, -2, 1));
        TextView play = makeText("▶", 20, TEXT, true);
        play.setGravity(Gravity.CENTER);
        play.setBackground(buttonBackground(ACCENT, dp(22)));
        banner.addView(play, new LinearLayout.LayoutParams(dp(44), dp(44)));
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(-1, -2);
        params.topMargin = dp(14);
        continueSlot.addView(banner, params);
    }

    private int statusColor(int status) {
        switch (status) {
            case Library.STATUS_PLAYING:
                return ACCENT;
            case Library.STATUS_COMPLETED:
                return 0xFF2E9E5B;
            case Library.STATUS_SHELVED:
                return 0xFF6B7280;
            default:
                return 0;
        }
    }

    /**
     * A game's page: its poster, play time, progress and favourite, and
     * everything that can be done with it.
     */
    private void showDetails(final App app) {
        final Dialog dialog = new Dialog(this, android.R.style.Theme_Material_NoActionBar_Fullscreen);
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        int pad = dp(20);
        layout.setPadding(pad, pad, pad, pad);

        LinearLayout bar = new LinearLayout(this);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        TextView title = makeText(app.name, 22, TEXT, true);
        title.setSingleLine(true);
        title.setEllipsize(TextUtils.TruncateAt.END);
        bar.addView(title, new LinearLayout.LayoutParams(0, -2, 1));
        bar.addView(makeButton(getString(R.string.close), false, v -> dialog.dismiss()),
            new LinearLayout.LayoutParams(-2, dp(40)));
        layout.addView(bar);

        LinearLayout top = new LinearLayout(this);
        top.setOrientation(LinearLayout.HORIZONTAL);
        top.setPadding(0, dp(16), 0, 0);
        ImageView poster = new ImageView(this);
        poster.setImageBitmap(app.poster);
        poster.setScaleType(ImageView.ScaleType.CENTER_CROP);
        GradientDrawable shape = new GradientDrawable();
        shape.setCornerRadius(dp(10));
        poster.setBackground(shape);
        poster.setClipToOutline(true);
        top.addView(poster, new LinearLayout.LayoutParams(dp(138),
            dp(138) * Posters.HEIGHT / Posters.WIDTH));
        LinearLayout info = new LinearLayout(this);
        info.setOrientation(LinearLayout.VERTICAL);
        info.setPadding(dp(18), 0, 0, 0);
        TextView lastPlayed = makeText(app.lastPlayed > 0
            ? getString(R.string.last_played, Library.ago(this, app.lastPlayed))
            : getString(R.string.never_played), 15, TEXT, false);
        info.addView(lastPlayed);
        if (app.playTime > 0) {
            TextView playTime = makeText(
                getString(R.string.play_time, Library.duration(this, app.playTime)),
                15, TEXT, false);
            playTime.setPadding(0, dp(6), 0, 0);
            info.addView(playTime);
        }
        final TextView favorite = makeButton("", false, null);
        Runnable showFavorite = () -> favorite.setText(app.favorite
            ? R.string.favorite_remove : R.string.favorite_add);
        showFavorite.run();
        favorite.setOnClickListener(v -> {
            app.favorite = !app.favorite;
            Library.setFavorite(this, app.key, app.favorite);
            showFavorite.run();
            refreshLibrary();
        });
        LinearLayout.LayoutParams favoriteParams = new LinearLayout.LayoutParams(-2, dp(40));
        favoriteParams.topMargin = dp(14);
        info.addView(favorite, favoriteParams);
        top.addView(info, new LinearLayout.LayoutParams(0, -2, 1));
        layout.addView(top);

        LinearLayout.LayoutParams playParams = new LinearLayout.LayoutParams(-1, dp(50));
        playParams.topMargin = dp(18);
        layout.addView(makeButton(getString(R.string.play), true, v -> {
            dialog.dismiss();
            launch(app);
        }), playParams);

        addSegments(layout, getString(R.string.library_status_label),
            getResources().getStringArray(R.array.library_status), app.status, i -> {
                app.status = i;
                Library.setStatus(this, app.key, i);
                refreshLibrary();
            });

        List<String> items = new ArrayList<>();
        List<Runnable> actions = new ArrayList<>();
        appActions(app, items, actions);
        for (int i = 0; i < items.size(); i++) {
            final Runnable action = actions.get(i);
            TextView button = makeButton(items.get(i), false, v -> {
                dialog.dismiss();
                action.run();
            });
            button.setGravity(Gravity.CENTER_VERTICAL | Gravity.START);
            LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, dp(46));
            params.topMargin = dp(i == 0 ? 18 : 8);
            layout.addView(button, params);
        }

        ScrollView scroll = new ScrollView(this);
        scroll.setBackgroundColor(0xFF15192E);
        scroll.addView(layout);
        dialog.setContentView(scroll);
        dialog.show();
    }

    private class AppAdapter extends BaseAdapter {
        @Override
        public int getCount() {
            return apps.size();
        }

        @Override
        public Object getItem(int position) {
            return apps.get(position);
        }

        @Override
        public long getItemId(int position) {
            return position;
        }

        @Override
        public View getView(int position, View convertView, ViewGroup parent) {
            LinearLayout item;
            if (convertView instanceof LinearLayout) {
                item = (LinearLayout) convertView;
            } else {
                item = new LinearLayout(LauncherActivity.this);
                item.setOrientation(LinearLayout.VERTICAL);
                FrameLayout frame = new FrameLayout(LauncherActivity.this);
                frame.setClipChildren(false);
                ImageView poster = new PosterView(LauncherActivity.this);
                poster.setScaleType(ImageView.ScaleType.CENTER_CROP);
                GradientDrawable shape = new GradientDrawable();
                shape.setColor(0x40FFFFFF);
                shape.setCornerRadius(dp(10));
                poster.setBackground(shape);
                poster.setClipToOutline(true);
                poster.setElevation(dp(4));
                // Ripple on top of the picture when pressed.
                poster.setForeground(new RippleDrawable(
                    ColorStateList.valueOf(0x40FFFFFF), null, shape));
                frame.addView(poster, new FrameLayout.LayoutParams(-1, -2));
                // Progress, top left, and favourite, top right.
                TextView status = makeText("", 11, TEXT, true);
                status.setPadding(dp(7), dp(2), dp(7), dp(2));
                status.setElevation(dp(5));
                FrameLayout.LayoutParams statusParams = new FrameLayout.LayoutParams(-2, -2,
                    Gravity.TOP | Gravity.START);
                statusParams.setMargins(dp(6), dp(6), 0, 0);
                frame.addView(status, statusParams);
                TextView star = makeText("★", 18, 0xFFFFD34D, true);
                star.setShadowLayer(dp(3), 0, dp(1), 0xCC000000);
                star.setElevation(dp(5));
                FrameLayout.LayoutParams starParams = new FrameLayout.LayoutParams(-2, -2,
                    Gravity.TOP | Gravity.END);
                starParams.setMargins(0, dp(2), dp(7), 0);
                frame.addView(star, starParams);
                item.addView(frame, new LinearLayout.LayoutParams(-1, -2));

                TextView name = makeText("", 13, TEXT, false);
                name.setGravity(Gravity.CENTER);
                name.setSingleLine(true);
                name.setEllipsize(TextUtils.TruncateAt.END);
                name.setShadowLayer(dp(2), 0, dp(1), 0x99000000);
                LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
                params.topMargin = dp(6);
                item.addView(name, params);
                // Always present, so cards in a row line up.
                TextView played = makeText("", 11, TEXT_DIM, false);
                played.setGravity(Gravity.CENTER);
                played.setSingleLine(true);
                played.setEllipsize(TextUtils.TruncateAt.END);
                item.addView(played, new LinearLayout.LayoutParams(-1, -2));
            }
            FrameLayout frame = (FrameLayout) item.getChildAt(0);
            ImageView poster = (ImageView) frame.getChildAt(0);
            TextView status = (TextView) frame.getChildAt(1);
            TextView star = (TextView) frame.getChildAt(2);
            TextView name = (TextView) item.getChildAt(1);
            TextView played = (TextView) item.getChildAt(2);
            App app = apps.get(position);
            poster.setImageBitmap(app.poster);
            int color = statusColor(app.status);
            status.setVisibility(color != 0 ? View.VISIBLE : View.GONE);
            if (color != 0) {
                status.setText(getResources().getStringArray(R.array.library_status)[app.status]);
                status.setBackground(buttonBackground(color, dp(8)));
            }
            star.setVisibility(app.favorite ? View.VISIBLE : View.GONE);
            name.setText(app.name);
            String text = playedText(app);
            played.setText(text.isEmpty() ? " " : text);
            return item;
        }
    }

    /// An image view as tall as a poster (3:4) for its width.
    private static class PosterView extends ImageView {
        PosterView(android.content.Context context) {
            super(context);
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            int width = View.MeasureSpec.getSize(widthSpec);
            setMeasuredDimension(width, width * Posters.HEIGHT / Posters.WIDTH);
        }
    }

    // ---- Actions ----

    private void launch(App app) {
        launch(app, null);
    }

    private void launch(App app, String performanceMode) {
        launch(app, performanceMode, null);
    }

    private void launch(App app, String performanceMode, String audioMode) {
        List<String> args = new ArrayList<>();
        args.add(app.path);
        args.addAll(quickOptions.toArgs());
        String widePref = expandedViewPref(app.bundleId);
        if (widePref != null
                && getSharedPreferences(PREFS, MODE_PRIVATE).getBoolean(widePref, false)) {
            args.add("--p2g-wide-screen");
        }
        String profile = appDeviceProfile(app);
        args.add("--device-profile=" + (profile != null ? profile : globalDeviceProfile()));
        if (performanceMode != null) {
            args.add("--device-profile=iPhone4,1");
            args.add("--performance-test=" + performanceMode);
        }
        if (audioMode != null) {
            args.add("--audio-test=" + audioMode);
        }
        if (app.key == null) {
            app.key = Library.key(app.bundleId, app.path);
        }
        Library.started(this, app.key);
        Intent intent = new Intent(this, MainActivity.class);
        intent.putExtra(MainActivity.EXTRA_ARGS, args.toArray(new String[0]));
        intent.putExtra(MainActivity.EXTRA_LIBRARY_KEY, app.key);
        startActivity(intent);
    }

    /// What can be done with a game, as titles and actions.
    private void appActions(final App app, List<String> items, List<Runnable> actions) {
        // Saves need to know which game they belong to.
        final boolean identified = app.bundleId != null && !app.bundleId.isEmpty();
        final Runnable unidentified = () ->
            Toast.makeText(this, R.string.unknown_game_saves, Toast.LENGTH_LONG).show();
        items.add(getString(R.string.menu_add_to_home));
        actions.add(() -> addShortcut(app));
        items.add(getString(R.string.menu_import_save));
        actions.add(identified ? () -> confirmImportSave(app) : unidentified);
        items.add(getString(R.string.menu_export_save));
        actions.add(identified ? () -> startExportSave(app) : unidentified);
        items.add(getString(R.string.menu_delete_game));
        actions.add(() -> confirmDelete(app));
        items.add(getString(R.string.menu_device_profile, appDeviceProfileLabel(app)));
        actions.add(() -> showDeviceProfiles(app, null));
        if ("com.rockstargames.gta3vc".equals(app.bundleId)) {
            items.add(getString(R.string.menu_performance_tests));
            actions.add(() -> showPerformanceTests(app));
            items.add(getString(R.string.menu_audio_tests));
            actions.add(() -> showAudioTests(app));
        } else if (MHP2G_BUNDLE_ID.equals(app.bundleId)) {
            items.add(getString(R.string.menu_p2g_layout));
            actions.add(() -> showP2GViewOptions(app));
            items.add(getString(R.string.menu_import_psp_save));
            actions.add(() -> confirmImportPspSave(app));
        } else if (MHFU_BUNDLE_ID.equals(app.bundleId)) {
            items.add(getString(R.string.menu_p2g_layout));
            actions.add(() -> showP2GViewOptions(app));
            items.add(getString(R.string.menu_import_psp_save));
            actions.add(() -> confirmImportPspSave(app));
        }
    }

    // ---- Home screen shortcuts ----

    /// Shortcuts open the launcher with this action, which starts the game
    /// straight away.
    private static final String ACTION_PLAY = "org.touchhle.android.action.PLAY";
    private static final String EXTRA_APP_PATH = "org.touchhle.android.APP_PATH";
    private static final String EXTRA_BUNDLE_ID = "org.touchhle.android.BUNDLE_ID";
    private static final String EXTRA_APP_NAME = "org.touchhle.android.APP_NAME";

    private void addShortcut(App app) {
        Intent play = new Intent(this, LauncherActivity.class)
            .setAction(ACTION_PLAY)
            .putExtra(EXTRA_APP_PATH, app.path)
            .putExtra(EXTRA_BUNDLE_ID, app.bundleId)
            .putExtra(EXTRA_APP_NAME, app.name);
        Bitmap icon = app.icon != null ? app.icon : app.poster;
        android.content.pm.ShortcutManager manager =
            getSystemService(android.content.pm.ShortcutManager.class);
        if (manager == null || !manager.isRequestPinShortcutSupported()) {
            Toast.makeText(this, R.string.shortcut_unsupported, Toast.LENGTH_LONG).show();
            return;
        }
        String id = "game:" + (app.bundleId.isEmpty() ? app.path : app.bundleId);
        android.content.pm.ShortcutInfo info =
            new android.content.pm.ShortcutInfo.Builder(this, id)
                .setShortLabel(app.name)
                .setIcon(android.graphics.drawable.Icon.createWithBitmap(icon))
                .setIntent(play)
                .build();
        // The home screen asks for confirmation and says when it's added.
        manager.requestPinShortcut(info, null);
    }

    /**
     * Starts the game a shortcut is for. Returns false if the intent isn't
     * from a shortcut, or the game is gone (which is pointed out).
     */
    private boolean playFromShortcut(Intent intent) {
        if (intent == null || !ACTION_PLAY.equals(intent.getAction())) {
            return false;
        }
        String path = intent.getStringExtra(EXTRA_APP_PATH);
        if (path == null || !new File(path).exists()) {
            Toast.makeText(this, R.string.shortcut_game_missing, Toast.LENGTH_LONG).show();
            return false;
        }
        App app = new App();
        app.path = path;
        app.bundleId = intent.getStringExtra(EXTRA_BUNDLE_ID);
        if (app.bundleId == null) {
            app.bundleId = "";
        }
        app.name = intent.getStringExtra(EXTRA_APP_NAME);
        launch(app);
        return true;
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        playFromShortcut(intent);
    }

    private void showP2GViewOptions(App app) {
        SharedPreferences prefs = getSharedPreferences(PREFS, MODE_PRIVATE);
        final String pref = expandedViewPref(app.bundleId);
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle(getString(R.string.p2g_layout_title, app.name))
            .setSingleChoiceItems(new String[]{getString(R.string.p2g_standard),
                    getString(R.string.p2g_wide)},
                prefs.getBoolean(pref, false) ? 1 : 0,
                (dialog, which) -> {
                    prefs.edit().putBoolean(pref, which == 1).apply();
                    dialog.dismiss();
                })
            .setNegativeButton(R.string.cancel, null)
            .show();
    }

    private static final String PERFORMANCE_REPORT = "touchHLE_performance_tests.txt";

    private void showPerformanceTests(final App app) {
        final String[] modes = {"reference", "low-resolution", "quiet-log", "low-resolution-quiet-log"};
        String[] choices = {getString(R.string.perf_choice_a), getString(R.string.perf_choice_b),
            getString(R.string.perf_choice_c), getString(R.string.perf_choice_d),
            getString(R.string.perf_view_reports), getString(R.string.perf_clear_reports)};
        LinearLayout title = new LinearLayout(this);
        title.setOrientation(LinearLayout.VERTICAL);
        title.setPadding(dp(24), dp(20), dp(24), dp(10));
        title.addView(makeText(getString(R.string.perf_title), 20, TEXT, true));
        title.addView(makeText(getString(R.string.perf_description), 13, TEXT_DIM, false));
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setCustomTitle(title)
            .setItems(choices, (dialog, which) -> {
                if (which < modes.length) {
                    launch(app, modes[which]);
                } else if (which == modes.length) {
                    showPerformanceReport();
                } else {
                    new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
                        .setTitle(R.string.perf_clear_title)
                        .setMessage(R.string.perf_clear_message)
                        .setPositiveButton(R.string.clear, (d, w) -> {
                            File report = new File(getBaseDir(), PERFORMANCE_REPORT);
                            boolean cleared = !report.exists() || report.delete();
                            Toast.makeText(this, cleared ? R.string.reports_cleared : R.string.clear_failed,
                                Toast.LENGTH_SHORT).show();
                        })
                        .setNegativeButton(R.string.cancel, null).show();
                }
            })
            .setNegativeButton(R.string.close, null).show();
    }

    private void showAudioTests(final App app) {
        final String[] modes = {"original", "smooth-edges", "lower-volume"};
        String[] choices = {getString(R.string.audio_original), getString(R.string.audio_smooth_edges),
            getString(R.string.audio_lower_volume), getString(R.string.audio_view_report)};
        LinearLayout title = new LinearLayout(this);
        title.setOrientation(LinearLayout.VERTICAL);
        title.setPadding(dp(24), dp(20), dp(24), dp(10));
        title.addView(makeText(getString(R.string.audio_title), 20, TEXT, true));
        title.addView(makeText(
            getString(R.string.audio_description, getString(R.string.mark_noise)),
            13, TEXT_DIM, false));
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setCustomTitle(title)
            .setItems(choices, (dialog, which) -> {
                if (which < modes.length) {
                    launch(app, null, modes[which]);
                } else {
                    showPerformanceReport();
                }
            })
            .setNegativeButton(R.string.close, null).show();
    }

    private void showPerformanceReport() {
        byte[] bytes = readFile(new File(getBaseDir(), PERFORMANCE_REPORT), 64 * 1024);
        if (bytes == null || bytes.length == 0) {
            Toast.makeText(this, R.string.no_test_results, Toast.LENGTH_LONG).show();
            return;
        }
        final String report = "Device: " + android.os.Build.MANUFACTURER + " " + android.os.Build.MODEL
            + "\nAndroid: " + android.os.Build.VERSION.RELEASE
            + "\nCPU/GL/present values are host wall time, not GPU execution time. "
            + "Readback is part of present time. Instrumentation adds overhead.\n\n"
            + new String(bytes, StandardCharsets.UTF_8);
        showTextDialog(getString(R.string.test_report_title), report, getString(R.string.share), () -> {
            Intent intent = new Intent(Intent.ACTION_SEND);
            intent.setType("text/plain");
            intent.putExtra(Intent.EXTRA_TEXT, report);
            try {
                startActivity(Intent.createChooser(intent, getString(R.string.share_test_report)));
            } catch (ActivityNotFoundException e) {
                Toast.makeText(this, R.string.no_share_app, Toast.LENGTH_LONG).show();
            }
        });
    }

    private void confirmDelete(final App app) {
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle(getString(R.string.delete_title, app.name))
            .setMessage(R.string.delete_message)
            .setPositiveButton(R.string.delete, (dialog, which) -> {
                if (deleteRecursively(new File(app.path))) {
                    Toast.makeText(this, R.string.deleted, Toast.LENGTH_SHORT).show();
                } else {
                    Toast.makeText(this, R.string.delete_failed, Toast.LENGTH_LONG).show();
                }
                rescan();
            })
            .setNegativeButton(R.string.cancel, null)
            .show();
    }

    private static boolean deleteRecursively(File file) {
        File[] children = file.isDirectory() ? file.listFiles() : null;
        if (children != null) {
            for (File child : children) {
                deleteRecursively(child);
            }
        }
        return file.delete();
    }

    private void showLog() {
        byte[] tail = readFile(new File(getBaseDir(), LOG_FILE), 48 * 1024);
        final String log = tail != null ? new String(tail, StandardCharsets.UTF_8) : "";
        showTextDialog(getString(R.string.log_title, LOG_FILE), log, getString(R.string.share), () -> {
            Intent intent = new Intent(Intent.ACTION_SEND);
            intent.setType("text/plain");
            intent.putExtra(Intent.EXTRA_TEXT, log);
            try {
                startActivity(Intent.createChooser(intent, getString(R.string.share_log)));
            } catch (ActivityNotFoundException e) {
                Toast.makeText(this, R.string.no_share_app, Toast.LENGTH_LONG).show();
            }
        });
    }

    private void showCopyright() {
        showTextDialog(getString(R.string.copyright),
            new String(nativeCopyrightText(getAssets()), StandardCharsets.UTF_8), null, null);
    }

    /** A full-screen dialog showing a long monospace text. */
    private void showTextDialog(String title, String text, String extraButton, Runnable extra) {
        final Dialog dialog = new Dialog(this, android.R.style.Theme_Material_NoActionBar_Fullscreen);
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        layout.setBackgroundColor(0xFF15192E);
        int pad = dp(16);
        layout.setPadding(pad, pad, pad, pad);

        LinearLayout bar = new LinearLayout(this);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        bar.addView(makeText(title, 18, TEXT, true), new LinearLayout.LayoutParams(0, -2, 1));
        if (extraButton != null) {
            LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-2, dp(40));
            params.rightMargin = dp(10);
            bar.addView(makeButton(extraButton, false, v -> extra.run()), params);
        }
        bar.addView(makeButton(getString(R.string.close), true, v -> dialog.dismiss()),
            new LinearLayout.LayoutParams(-2, dp(40)));
        layout.addView(bar);

        ScrollView scroll = new ScrollView(this);
        TextView body = makeText(text, 12, 0xE6FFFFFF, false);
        body.setTypeface(Typeface.MONOSPACE);
        body.setPadding(0, dp(12), 0, dp(12));
        scroll.addView(body);
        layout.addView(scroll, new LinearLayout.LayoutParams(-1, 0, 1));
        if (extra != null) {
            // Logs are most useful from the end.
            scroll.post(() -> scroll.fullScroll(View.FOCUS_DOWN));
        }

        dialog.setContentView(layout);
        dialog.show();
    }

    // ---- Quick options ----

    private static int deviceProfileIndex(String value) {
        for (int i = 0; i < DEVICE_PROFILES.length; i++) {
            if (DEVICE_PROFILES[i].equals(value)) {
                return i;
            }
        }
        return -1;
    }

    /// Unknown or wrongly typed old preferences behave like a missing choice.
    private String readDeviceProfile(String key) {
        try {
            String value = mergedDeviceProfile(
                getSharedPreferences(PREFS, MODE_PRIVATE).getString(key, null));
            return deviceProfileIndex(value) >= 0 ? value : null;
        } catch (ClassCastException e) {
            return null;
        }
    }

    private String globalDeviceProfile() {
        String value = readDeviceProfile(PREF_DEVICE_PROFILE);
        return value != null ? value : "auto";
    }

    private String appDeviceProfileKey(App app) {
        // Bundle IDs survive reimporting or renaming the game's IPA. Namespace
        // the path fallback so it cannot collide with a bundle identifier.
        return PREF_APP_DEVICE_PROFILE + (app.bundleId != null && !app.bundleId.isEmpty()
            ? "bundle:" + app.bundleId : "path:" + app.path);
    }

    private String appDeviceProfile(App app) {
        return readDeviceProfile(appDeviceProfileKey(app));
    }

    private String[] deviceProfileLabels() {
        return getResources().getStringArray(R.array.device_profile_labels);
    }

    private String deviceProfileLabel(String profile) {
        int index = deviceProfileIndex(profile);
        return deviceProfileLabels()[index >= 0 ? index : 0];
    }

    private String appDeviceProfileLabel(App app) {
        String profile = appDeviceProfile(app);
        return profile != null ? deviceProfileLabel(profile)
            : getString(R.string.device_profile_use_global, deviceProfileLabel(globalDeviceProfile()));
    }

    /// Single-choice lists scroll on smaller phones; no cramped row of models.
    private void showDeviceProfiles(final App app, final Runnable onChanged) {
        String global = globalDeviceProfile();
        String selected = app != null ? appDeviceProfile(app) : global;
        int offset = app != null ? 1 : 0;
        String[] choices = new String[DEVICE_PROFILES.length + offset];
        if (app != null) {
            choices[0] = getString(R.string.device_profile_use_global, deviceProfileLabel(global));
        }
        System.arraycopy(deviceProfileLabels(), 0, choices, offset, DEVICE_PROFILES.length);

        LinearLayout title = new LinearLayout(this);
        title.setOrientation(LinearLayout.VERTICAL);
        title.setPadding(dp(24), dp(20), dp(24), dp(8));
        title.addView(makeText(getString(R.string.device_profile_title), 20, TEXT, true));
        TextView current = makeText(getString(R.string.device_profile_global, deviceProfileLabel(global))
            + (app != null
                ? "\n" + getString(R.string.device_profile_this_game, appDeviceProfileLabel(app))
                : ""),
            14, TEXT_DIM, false);
        current.setPadding(0, dp(10), 0, dp(6));
        title.addView(current);
        title.addView(makeText(getString(R.string.device_profile_hint), 13, TEXT_DIM, false));

        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setCustomTitle(title)
            .setSingleChoiceItems(choices, selected == null ? 0 : deviceProfileIndex(selected) + offset,
                (dialog, which) -> {
                    SharedPreferences.Editor editor = getSharedPreferences(PREFS, MODE_PRIVATE).edit();
                    String key = app != null ? appDeviceProfileKey(app) : PREF_DEVICE_PROFILE;
                    if (app != null && which == 0) {
                        editor.remove(key);
                    } else {
                        editor.putString(key, DEVICE_PROFILES[which - offset]);
                    }
                    editor.apply();
                    if (onChanged != null) {
                        onChanged.run();
                    }
                    dialog.dismiss();
                })
            .setNegativeButton(R.string.cancel, null)
            .show();
    }

    private void showQuickOptions() {
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        int pad = dp(20);
        layout.setPadding(pad, dp(8), pad, dp(8));

        final TextView deviceProfile = makeText(
            getString(R.string.menu_device_profile, deviceProfileLabel(globalDeviceProfile())),
            15, TEXT, true);
        deviceProfile.setPadding(dp(14), dp(12), dp(14), dp(12));
        deviceProfile.setMinHeight(dp(48));
        deviceProfile.setBackground(buttonBackground(0x2EFFFFFF, dp(12)));
        deviceProfile.setOnClickListener(v -> showDeviceProfiles(null,
            () -> deviceProfile.setText(getString(R.string.menu_device_profile,
                deviceProfileLabel(globalDeviceProfile())))));
        layout.addView(deviceProfile, new LinearLayout.LayoutParams(-1, -2));
        TextView deviceHint = makeText(getString(R.string.device_profile_hint), 13, TEXT_DIM, false);
        deviceHint.setPadding(0, dp(6), 0, 0);
        layout.addView(deviceHint);

        addSegments(layout, getString(R.string.option_filter),
            getResources().getStringArray(R.array.option_filter_choices),
            quickOptions.filter, i -> quickOptions.filter = i);
        addSegments(layout, getString(R.string.option_aspect),
            getResources().getStringArray(R.array.option_aspect_choices),
            quickOptions.aspect, i -> quickOptions.aspect = i);
        addSegments(layout, getString(R.string.option_orientation),
            getResources().getStringArray(R.array.option_orientation_choices),
            quickOptions.orientation, i -> quickOptions.orientation = i);
        addSegments(layout, getString(R.string.option_scale),
            getResources().getStringArray(R.array.option_scale_choices),
            quickOptions.scaleHack, i -> quickOptions.scaleHack = i);
        addSwitch(layout, getString(R.string.option_keep_screen_on), quickOptions.keepScreenOn,
            b -> quickOptions.keepScreenOn = b);
        addSwitch(layout, getString(R.string.option_tilt_controls), quickOptions.tiltControls,
            b -> quickOptions.tiltControls = b);
        addSwitch(layout, getString(R.string.option_network), quickOptions.network,
            b -> quickOptions.network = b);
        // A launcher setting, not a touchHLE option, so it's not in the file.
        final SharedPreferences prefs = getSharedPreferences(PREFS, MODE_PRIVATE);
        addSwitch(layout, getString(R.string.option_check_updates),
            prefs.getBoolean(PREF_CHECK_UPDATES, true),
            b -> prefs.edit().putBoolean(PREF_CHECK_UPDATES, b).apply());
        addSwitch(layout, getString(R.string.option_online_posters),
            prefs.getBoolean(PREF_ONLINE_POSTERS, true),
            b -> prefs.edit().putBoolean(PREF_ONLINE_POSTERS, b).apply());

        TextView language = makeButton(
            getString(R.string.language, Languages.name(this, Languages.chosenTag(this))), false, null);
        LinearLayout.LayoutParams languageParams = new LinearLayout.LayoutParams(-1, dp(44));
        languageParams.topMargin = dp(16);
        layout.addView(language, languageParams);

        TextView copyright = makeButton(getString(R.string.copyright), false, null);
        LinearLayout.LayoutParams copyrightParams = new LinearLayout.LayoutParams(-1, dp(44));
        copyrightParams.topMargin = dp(16);
        layout.addView(copyright, copyrightParams);

        ScrollView scroll = new ScrollView(this);
        scroll.addView(layout);
        final AlertDialog dialog = new AlertDialog.Builder(
                this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle(R.string.settings)
            .setView(scroll)
            .setPositiveButton(R.string.done, null)
            .setOnDismissListener(d -> quickOptions.save(getBaseDir()))
            .create();
        language.setOnClickListener(v -> showLanguages(dialog));
        copyright.setOnClickListener(v -> showCopyright());
        dialog.show();
    }

    /// Changing the language closes the settings and rebuilds the launcher in
    /// the new language.
    private void showLanguages(final Dialog settings) {
        final String current = Languages.chosenTag(this);
        int selected = 0;
        for (int i = 0; i < Languages.TAGS.length; i++) {
            if (Languages.TAGS[i].equals(current)) {
                selected = i;
            }
        }
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle(R.string.language_title)
            .setSingleChoiceItems(Languages.names(this), selected, (dialog, which) -> {
                dialog.dismiss();
                if (Languages.TAGS[which].equals(current)) {
                    return;
                }
                Languages.choose(this, Languages.TAGS[which]);
                settings.dismiss();
                recreate();
            })
            .setNegativeButton(R.string.cancel, null)
            .show();
    }

    private interface IntSetter {
        void set(int value);
    }

    private interface BoolSetter {
        void set(boolean value);
    }

    private void addSegments(LinearLayout parent, String label, String[] titles, int selected,
                             final IntSetter setter) {
        TextView labelView = makeText(label, 14, TEXT_DIM, false);
        labelView.setPadding(0, dp(14), 0, dp(6));
        parent.addView(labelView);

        final LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        for (int i = 0; i < titles.length; i++) {
            final int index = i;
            TextView segment = makeText(titles[i], 14, TEXT, false);
            segment.setGravity(Gravity.CENTER);
            segment.setSingleLine(true);
            segment.setOnClickListener(v -> {
                setter.set(index);
                styleSegments(row, index);
            });
            LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(0, dp(38), 1);
            if (i > 0) {
                params.leftMargin = dp(6);
            }
            row.addView(segment, params);
        }
        styleSegments(row, selected);
        parent.addView(row);
    }

    private void styleSegments(LinearLayout row, int selected) {
        for (int i = 0; i < row.getChildCount(); i++) {
            TextView segment = (TextView) row.getChildAt(i);
            boolean on = i == selected;
            segment.setBackground(buttonBackground(on ? ACCENT : 0x26FFFFFF, dp(10)));
            segment.setTypeface(on ? Typeface.DEFAULT_BOLD : Typeface.DEFAULT);
        }
    }

    private void addSwitch(LinearLayout parent, String label, boolean checked,
                           final BoolSetter setter) {
        Switch view = new Switch(this);
        view.setText(label);
        view.setTextSize(TypedValue.COMPLEX_UNIT_SP, 15);
        view.setChecked(checked);
        view.setMinHeight(dp(48));
        view.setOnCheckedChangeListener((button, isChecked) -> setter.set(isChecked));
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
        params.topMargin = dp(8);
        parent.addView(view, params);
    }

    /**
     * The options from the "quick options" dialog. They're remembered in the
     * same file, and turned into the same command-line options, as in the app
     * picker (src/environment/app_picker.rs).
     */
    static class QuickOptions {
        static final String FILE = "touchHLE_quick_options.txt";
        static final String[] ASPECT = {null, "stretch", "fill", "integer"};
        static final String[] FILTER = {"smooth", "sharp", "pixel", "hq"};
        static final String[] ORIENTATION =
            {null, "--landscape-left", "--landscape-right", "--upside-down"};

        int scaleHack = 0; // 0: default, otherwise the --scale-hack= value
        int orientation = 0;
        int aspect = 0;
        int filter = 1; // sharp is the default on Android (see src/options.rs)
        boolean keepScreenOn = true;
        boolean tiltControls = true;
        boolean network = false;

        static int indexOf(String[] values, String value) {
            for (int i = 0; i < values.length; i++) {
                if (value.equals(values[i])) {
                    return i;
                }
            }
            return -1;
        }

        static QuickOptions load(File baseDir) {
            QuickOptions options = new QuickOptions();
            byte[] bytes = readFile(new File(baseDir, FILE), 64 * 1024);
            if (bytes == null) {
                return options;
            }
            for (String arg : new String(bytes, StandardCharsets.UTF_8).split("\n")) {
                arg = arg.trim();
                try {
                    if (arg.startsWith("--scale-hack=")) {
                        int value = Integer.parseInt(arg.substring("--scale-hack=".length()));
                        options.scaleHack = value >= 1 && value <= 4 ? value : 0;
                    } else if (arg.startsWith("--aspect=")) {
                        options.aspect = Math.max(0, indexOf(ASPECT, arg.substring(9)));
                    } else if (arg.startsWith("--filter=")) {
                        int i = indexOf(FILTER, arg.substring(9));
                        options.filter = i >= 0 ? i : options.filter;
                    } else if (indexOf(ORIENTATION, arg) >= 0) {
                        options.orientation = indexOf(ORIENTATION, arg);
                    } else if (arg.equals("--disable-analog-stick-tilt-controls")) {
                        options.tiltControls = false;
                    } else if (arg.equals("--allow-network-access")) {
                        options.network = true;
                    } else if (arg.equals("--keep-screen-on")) {
                        options.keepScreenOn = true;
                    } else if (arg.equals("--no-keep-screen-on")) {
                        options.keepScreenOn = false;
                    }
                } catch (NumberFormatException e) {
                    // Ignore it.
                }
            }
            return options;
        }

        List<String> toArgs() {
            List<String> args = new ArrayList<>();
            if (scaleHack != 0) {
                args.add("--scale-hack=" + scaleHack);
            }
            if (ORIENTATION[orientation] != null) {
                args.add(ORIENTATION[orientation]);
            }
            if (ASPECT[aspect] != null) {
                args.add("--aspect=" + ASPECT[aspect]);
            }
            args.add("--filter=" + FILTER[filter]);
            if (!tiltControls) {
                args.add("--disable-analog-stick-tilt-controls");
            }
            if (network) {
                args.add("--allow-network-access");
            }
            args.add(keepScreenOn ? "--keep-screen-on" : "--no-keep-screen-on");
            return args;
        }

        void save(File baseDir) {
            try (OutputStream out = new FileOutputStream(new File(baseDir, FILE))) {
                out.write(TextUtils.join("\n", toArgs()).getBytes(StandardCharsets.UTF_8));
            } catch (Exception e) {
                // Not essential.
            }
        }
    }

    // ---- Importing ----

    private void startImport() {
        Intent[] attempts = {pickIntent(Intent.ACTION_OPEN_DOCUMENT),
                             pickIntent(Intent.ACTION_GET_CONTENT)};
        for (Intent intent : attempts) {
            try {
                startActivityForResult(intent, REQUEST_IMPORT);
                return;
            } catch (ActivityNotFoundException | SecurityException e) {
                // Try the next one.
            }
        }
        Toast.makeText(this, R.string.no_file_picker, Toast.LENGTH_LONG).show();
    }

    private static Intent pickIntent(String action) {
        Intent intent = new Intent(action);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        // .ipa files have no well-known MIME type, so allow anything.
        intent.setType("*/*");
        intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
        return intent;
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == REQUEST_IMPORT_PSP_SAVE_FOLDER
            || requestCode == REQUEST_IMPORT_PSP_SAVE_FILES) {
            if (resultCode == RESULT_OK && data != null && saveBundleId != null) {
                List<Uri> files = new ArrayList<>();
                ClipData clip = data.getClipData();
                if (clip != null) {
                    for (int i = 0; i < clip.getItemCount(); i++) {
                        files.add(clip.getItemAt(i).getUri());
                    }
                } else if (data.getData() != null) {
                    files.add(data.getData());
                }
                boolean folder = requestCode == REQUEST_IMPORT_PSP_SAVE_FOLDER;
                if (folder || !files.isEmpty()) {
                    importPspSave(folder ? data.getData() : null, files);
                }
            }
            return;
        }
        if (requestCode == REQUEST_IMPORT_SAVE || requestCode == REQUEST_EXPORT_SAVE) {
            if (resultCode == RESULT_OK && data != null && data.getData() != null
                && saveBundleId != null) {
                if (requestCode == REQUEST_IMPORT_SAVE) {
                    importSave(data.getData());
                } else {
                    exportSave(data.getData());
                }
            }
            return;
        }
        if (requestCode != REQUEST_IMPORT || resultCode != RESULT_OK || data == null) {
            return;
        }
        final List<Uri> uris = new ArrayList<>();
        ClipData clip = data.getClipData();
        if (clip != null) {
            for (int i = 0; i < clip.getItemCount(); i++) {
                uris.add(clip.getItemAt(i).getUri());
            }
        } else if (data.getData() != null) {
            uris.add(data.getData());
        }
        if (uris.isEmpty()) {
            return;
        }

        loading = true;
        updateStatus();
        statusText.setText(R.string.importing);
        // Copying can take a while for big files, so don't block the UI thread.
        new Thread(() -> {
            final String message = importAll(uris);
            runOnUiThread(() -> {
                Toast.makeText(this, message, Toast.LENGTH_LONG).show();
                rescan();
            });
        }).start();
    }

    private String importAll(List<Uri> uris) {
        File appsDir = getAppsDir();
        appsDir.mkdirs();
        int imported = 0;
        List<String> skipped = new ArrayList<>();
        List<String> failed = new ArrayList<>();
        for (Uri uri : uris) {
            String name = displayName(uri);
            if (name == null || !name.toLowerCase(Locale.ROOT).endsWith(".ipa")) {
                skipped.add(name != null ? name : String.valueOf(uri.getLastPathSegment()));
                continue;
            }
            // Don't let a strange name escape the apps directory.
            name = name.replace('/', '_').replace('\\', '_');
            try {
                copy(uri, new File(appsDir, name));
                imported++;
            } catch (Exception e) {
                failed.add(name);
            }
        }

        String separator = getString(R.string.list_separator);
        StringBuilder message = new StringBuilder();
        message.append(getResources().getQuantityString(R.plurals.imported_games, imported, imported));
        if (!skipped.isEmpty()) {
            message.append('\n').append(
                getString(R.string.skipped_not_ipa, TextUtils.join(separator, skipped)));
        }
        if (!failed.isEmpty()) {
            message.append('\n').append(
                getString(R.string.import_failed, TextUtils.join(separator, failed)));
        }
        return message.toString();
    }

    private String displayName(Uri uri) {
        try (Cursor cursor = getContentResolver().query(
                uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst() && !cursor.isNull(0)) {
                return cursor.getString(0);
            }
        } catch (Exception e) {
            // Fall back to the URI below.
        }
        return uri.getLastPathSegment();
    }

    private void copy(Uri uri, File dest) throws Exception {
        // Copy to a temporary name first, so a partially copied file is never
        // listed (or run).
        File temp = new File(dest.getParentFile(), dest.getName() + ".importing");
        try (InputStream in = getContentResolver().openInputStream(uri);
             OutputStream out = new FileOutputStream(temp)) {
            if (in == null) {
                throw new Exception("Couldn't open " + uri);
            }
            byte[] buffer = new byte[1 << 16];
            int count;
            while ((count = in.read(buffer)) != -1) {
                out.write(buffer, 0, count);
            }
        } catch (Exception e) {
            temp.delete();
            throw e;
        }
        if (dest.exists() && !dest.delete()) {
            temp.delete();
            throw new Exception("Couldn't replace " + dest);
        }
        if (!temp.renameTo(dest)) {
            temp.delete();
            throw new Exception("Couldn't rename " + temp);
        }
    }

    // ---- Saves ----

    private File getSaveDir(String bundleId) {
        return new File(new File(getBaseDir(), SANDBOX_DIR), bundleId);
    }

    private static String timestamp() {
        return new SimpleDateFormat("yyyyMMdd_HHmmss", Locale.ROOT).format(new Date());
    }

    /// The parts of an app's data that make up its saves (not tmp or caches).
    private static boolean isSavePath(String relative) {
        return (relative.equals("Documents") || relative.startsWith("Documents/")
                || relative.equals("Library") || relative.startsWith("Library/"))
            && !relative.equals("Library/Caches") && !relative.startsWith("Library/Caches/");
    }

    /// Writes an app's saves to `out` as a zip. Returns the number of files
    /// written and (in `count[1]`) skipped because they couldn't be read.
    private int[] writeSaveZip(String bundleId, OutputStream out) throws Exception {
        File saveDir = getSaveDir(bundleId);
        int[] count = {0, 0};
        try (ZipOutputStream zip = new ZipOutputStream(out)) {
            zip.putNextEntry(new ZipEntry(SAVE_MARKER));
            zip.write(("touchHLE save\nbundle=" + bundleId + "\n").getBytes(StandardCharsets.UTF_8));
            zip.closeEntry();
            for (String top : new String[]{"Documents", "Library"}) {
                addToZip(zip, new File(saveDir, top), top, count);
            }
        }
        return count;
    }

    private static void addToZip(ZipOutputStream zip, File file, String relative, int[] count)
            throws Exception {
        if (!isSavePath(relative)) {
            return;
        }
        File[] children = file.listFiles();
        if (file.isDirectory()) {
            if (children != null) {
                for (File child : children) {
                    addToZip(zip, child, relative + "/" + child.getName(), count);
                }
            }
            return;
        }
        if (!file.isFile()) {
            return;
        }
        InputStream in;
        try {
            in = new FileInputStream(file);
        } catch (Exception e) {
            // E.g. a save copied in by hand that touchHLE isn't allowed to read.
            count[1]++;
            return;
        }
        try (InputStream input = in) {
            zip.putNextEntry(new ZipEntry(relative));
            byte[] buffer = new byte[1 << 16];
            int n;
            while ((n = input.read(buffer)) != -1) {
                zip.write(buffer, 0, n);
            }
            zip.closeEntry();
        }
        count[0]++;
    }

    private static boolean hasFiles(File dir) {
        File[] children = dir.listFiles();
        if (children == null) {
            return false;
        }
        for (File child : children) {
            if (child.isFile() || hasFiles(child)) {
                return true;
            }
        }
        return false;
    }

    private boolean hasSaves(String bundleId) {
        File saveDir = getSaveDir(bundleId);
        return hasFiles(new File(saveDir, "Documents")) || hasFiles(new File(saveDir, "Library"));
    }

    private void startExportSave(App app) {
        if (!hasSaves(app.bundleId)) {
            Toast.makeText(this, getString(R.string.no_saves_yet, app.name), Toast.LENGTH_LONG).show();
            return;
        }
        saveBundleId = app.bundleId;
        saveAppName = app.name;
        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("application/zip");
        intent.putExtra(Intent.EXTRA_TITLE, app.bundleId + "_save_" + timestamp() + ".zip");
        try {
            startActivityForResult(intent, REQUEST_EXPORT_SAVE);
        } catch (ActivityNotFoundException | SecurityException e) {
            // No system "save as" screen: save it in touchHLE's own folder.
            exportSaveToFolder();
        }
    }

    private void exportSave(final Uri uri) {
        final String bundleId = saveBundleId;
        final String name = saveAppName;
        new Thread(() -> {
            String message;
            try (OutputStream out = getContentResolver().openOutputStream(uri)) {
                if (out == null) {
                    throw new Exception("Couldn't open " + uri);
                }
                message = exportMessage(name, writeSaveZip(bundleId, out), "");
            } catch (Exception e) {
                message = getString(R.string.export_save_failed, e.getMessage());
            }
            final String text = message;
            runOnUiThread(() -> Toast.makeText(this, text, Toast.LENGTH_LONG).show());
        }).start();
    }

    private String exportMessage(String name, int[] count, String where) {
        String message = getResources().getQuantityString(
            R.plurals.exported_saves, count[0], name, count[0]) + where;
        if (count[1] > 0) {
            message += "\n" + getResources().getQuantityString(
                R.plurals.unreadable_files_skipped, count[1], count[1]);
        }
        return message;
    }

    private void exportSaveToFolder() {
        final String bundleId = saveBundleId;
        final String name = saveAppName;
        new Thread(() -> {
            String message;
            File dir = new File(getBaseDir(), SAVE_EXPORTS_DIR);
            File file = new File(dir, bundleId + "_save_" + timestamp() + ".zip");
            try {
                dir.mkdirs();
                int[] count;
                try (OutputStream out = new FileOutputStream(file)) {
                    count = writeSaveZip(bundleId, out);
                }
                message = exportMessage(name, count,
                    "\n" + getString(R.string.saved_in_folder,
                        SAVE_EXPORTS_DIR + "/" + file.getName()));
            } catch (Exception e) {
                file.delete();
                message = getString(R.string.export_save_failed, e.getMessage());
            }
            final String text = message;
            runOnUiThread(() -> Toast.makeText(this, text, Toast.LENGTH_LONG).show());
        }).start();
    }

    private void confirmImportSave(final App app) {
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle(getString(R.string.import_save_title, app.name))
            .setMessage(getString(R.string.import_save_message, SAVE_BACKUPS_DIR))
            .setPositiveButton(R.string.choose_file, (dialog, which) -> {
                saveBundleId = app.bundleId;
                saveAppName = app.name;
                Intent[] attempts = {new Intent(Intent.ACTION_OPEN_DOCUMENT),
                                     new Intent(Intent.ACTION_GET_CONTENT)};
                for (Intent intent : attempts) {
                    intent.addCategory(Intent.CATEGORY_OPENABLE);
                    intent.setType("*/*");
                    try {
                        startActivityForResult(intent, REQUEST_IMPORT_SAVE);
                        return;
                    } catch (ActivityNotFoundException | SecurityException e) {
                        // Try the next one.
                    }
                }
                Toast.makeText(this, R.string.no_file_picker, Toast.LENGTH_LONG).show();
            })
            .setNegativeButton(R.string.cancel, null)
            .show();
    }

    // ---- PSP saves (Monster Hunter Portable 2nd G, Freedom Unite) ----

    private void confirmImportPspSave(final App app) {
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle(getString(R.string.import_psp_save_title, app.name))
            .setMessage(getString(R.string.import_psp_save_message, SAVE_BACKUPS_DIR))
            .setPositiveButton(R.string.choose_folder, (dialog, which) -> {
                saveBundleId = app.bundleId;
                saveAppName = app.name;
                try {
                    startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE),
                        REQUEST_IMPORT_PSP_SAVE_FOLDER);
                } catch (ActivityNotFoundException | SecurityException e) {
                    Toast.makeText(this, R.string.no_file_picker, Toast.LENGTH_LONG).show();
                }
            })
            .setNeutralButton(R.string.choose_files, (dialog, which) -> {
                saveBundleId = app.bundleId;
                saveAppName = app.name;
                Intent[] attempts = {new Intent(Intent.ACTION_OPEN_DOCUMENT),
                                     new Intent(Intent.ACTION_GET_CONTENT)};
                for (Intent intent : attempts) {
                    intent.addCategory(Intent.CATEGORY_OPENABLE);
                    intent.setType("*/*");
                    intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
                    try {
                        startActivityForResult(intent, REQUEST_IMPORT_PSP_SAVE_FILES);
                        return;
                    } catch (ActivityNotFoundException | SecurityException e) {
                        // Try the next one.
                    }
                }
                Toast.makeText(this, R.string.no_file_picker, Toast.LENGTH_LONG).show();
            })
            .setNegativeButton(R.string.cancel, null)
            .show();
    }

    /// Finds MHP2NDG.BIN and PARAM.SFO in a chosen folder (or in one of its
    /// subfolders named like a PSP save, e.g. if PSP/SAVEDATA was chosen) or
    /// among chosen files. Returns {save, PARAM.SFO or null}.
    private Uri[] findPspSave(Uri tree, List<Uri> files) {
        Uri[] found = new Uri[2];
        if (tree != null) {
            String folder = DocumentsContract.getTreeDocumentId(tree);
            if (findChild(tree, folder, "MHP2NDG.BIN") == null) {
                for (String name : PSP_SAVE_FOLDERS) {
                    String subfolder = findChild(tree, folder, name);
                    if (subfolder != null) {
                        folder = subfolder;
                        break;
                    }
                }
            }
            found[0] = documentUri(tree, findChild(tree, folder, "MHP2NDG.BIN"));
            found[1] = documentUri(tree, findChild(tree, folder, "PARAM.SFO"));
        } else {
            for (Uri uri : files) {
                String name = displayName(uri);
                if ("PARAM.SFO".equalsIgnoreCase(name)) {
                    found[1] = uri;
                } else if (found[0] == null || "MHP2NDG.BIN".equalsIgnoreCase(name)) {
                    found[0] = uri;
                }
            }
        }
        return found;
    }

    /// The document ID of the child of `parent` called `name` (any case).
    private String findChild(Uri tree, String parent, String name) {
        Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parent);
        try (Cursor cursor = getContentResolver().query(children, new String[]{
                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                DocumentsContract.Document.COLUMN_DISPLAY_NAME}, null, null, null)) {
            while (cursor != null && cursor.moveToNext()) {
                if (name.equalsIgnoreCase(cursor.getString(1))) {
                    return cursor.getString(0);
                }
            }
        } catch (Exception e) {
            // Treated as missing.
        }
        return null;
    }

    private static Uri documentUri(Uri tree, String documentId) {
        return documentId == null ? null
            : DocumentsContract.buildDocumentUriUsingTree(tree, documentId);
    }

    private byte[] readAll(InputStream in) throws Exception {
        if (in == null) {
            throw new Exception("Couldn't open the file");
        }
        try (InputStream input = in; ByteArrayOutputStream out = new ByteArrayOutputStream()) {
            byte[] buffer = new byte[1 << 16];
            int n;
            while ((n = input.read(buffer)) != -1) {
                out.write(buffer, 0, n);
            }
            return out.toByteArray();
        }
    }

    /// Converts the chosen PSP save, backs up the current save, and puts the
    /// converted one in its place.
    private void importPspSave(final Uri tree, final List<Uri> files) {
        final String bundleId = saveBundleId;
        new Thread(() -> {
            String message;
            try {
                Uri[] found = findPspSave(tree, files);
                if (found[0] == null) {
                    throw new Exception(getString(R.string.psp_save_not_found));
                }
                byte[] save = readAll(getContentResolver().openInputStream(found[0]));
                byte[] sfo = found[1] == null ? null
                    : readAll(getContentResolver().openInputStream(found[1]));
                File documents = new File(getSaveDir(bundleId), "Documents");
                File current = new File(documents, "savedata.dat");
                byte[] currentBytes = null;
                if (current.isFile()) {
                    try {
                        currentBytes = readAll(new FileInputStream(current));
                    } catch (Exception e) {
                        // Then the default settings are used.
                    }
                }

                byte[] result = nativeImportPspSave(save, sfo, currentBytes);
                if (result == null || result.length == 0) {
                    throw new Exception("no result");
                }
                if (result[0] != 0) {
                    throw new Exception(new String(result, 1, result.length - 1,
                        StandardCharsets.UTF_8));
                }
                int namesLength = ByteBuffer.wrap(result, 1, 4)
                    .order(java.nio.ByteOrder.LITTLE_ENDIAN).getInt();
                String names = new String(result, 5, namesLength, StandardCharsets.UTF_8)
                    .replace("\n", getString(R.string.list_separator));

                // Write the new save next to the old one first, so nothing is
                // lost if that fails.
                documents.mkdirs();
                File temp = new File(documents, "savedata.dat.importing");
                try (OutputStream out = new FileOutputStream(temp)) {
                    out.write(result, 5 + namesLength, result.length - 5 - namesLength);
                }
                String backup = null;
                if (current.exists()) {
                    File backupDir = new File(new File(getBaseDir(), SAVE_BACKUPS_DIR),
                        bundleId + "_" + timestamp());
                    try {
                        moveAside(current, new File(new File(backupDir, "Documents"),
                            "savedata.dat"));
                    } catch (Exception e) {
                        temp.delete();
                        throw e;
                    }
                    backup = SAVE_BACKUPS_DIR + "/" + backupDir.getName();
                }
                if (!temp.renameTo(current)) {
                    throw new Exception("Couldn't rename " + temp);
                }
                message = backup == null
                    ? getString(R.string.imported_psp_save, names)
                    : getString(R.string.imported_psp_save_backup, names, backup);
            } catch (Exception e) {
                message = getString(R.string.import_psp_save_failed, e.getMessage());
            }
            final String text = message;
            runOnUiThread(() -> Toast.makeText(this, text, Toast.LENGTH_LONG).show());
        }).start();
    }

    /// Unpacks the zip into a fresh folder in the cache, then checks it and
    /// asks before replacing the app's saves with it.
    private void importSave(final Uri uri) {
        final String bundleId = saveBundleId;
        final String name = saveAppName;
        new Thread(() -> {
            final File staging = new File(getCacheDir(), "save_import");
            deleteRecursively(staging);
            String error = null;
            String zipBundleId = null;
            try {
                unzip(uri, staging);
                File root = saveRoot(staging);
                zipBundleId = readMarker(new File(staging, SAVE_MARKER));
                if (zipBundleId == null) {
                    zipBundleId = readMarker(new File(root, SAVE_MARKER));
                }
                if (!hasFiles(root)) {
                    error = getString(R.string.zip_has_no_saves);
                }
            } catch (Exception e) {
                error = getString(R.string.zip_unreadable);
            }
            final String failure = error;
            final String fromId = zipBundleId;
            runOnUiThread(() -> {
                if (failure != null) {
                    deleteRecursively(staging);
                    Toast.makeText(this, failure, Toast.LENGTH_LONG).show();
                } else if (fromId != null && !fromId.equals(bundleId)) {
                    new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
                        .setTitle(R.string.save_mismatch_title)
                        .setMessage(getString(R.string.save_mismatch_message, fromId, name, bundleId))
                        .setPositiveButton(R.string.import_anyway,
                            (d, w) -> applySave(staging, bundleId, name))
                        .setNegativeButton(R.string.cancel, (d, w) -> deleteRecursively(staging))
                        .show();
                } else {
                    applySave(staging, bundleId, name);
                }
            });
        }).start();
    }

    private void applySave(final File staging, final String bundleId, final String name) {
        new Thread(() -> {
            String message;
            File saveDir = getSaveDir(bundleId);
            String step = getString(R.string.step_move_old_saves);
            try {
                // The old saves are moved into the backup folder rather than
                // copied and deleted: saves copied in by hand (e.g. with a file
                // manager) may not be readable or deletable by touchHLE, but
                // its own folders can still be moved.
                File backup = new File(new File(getBaseDir(), SAVE_BACKUPS_DIR),
                    bundleId + "_" + timestamp());
                moveAside(new File(saveDir, "Documents"), new File(backup, "Documents"));
                File[] libraryChildren = new File(saveDir, "Library").listFiles();
                if (libraryChildren != null) {
                    for (File child : libraryChildren) {
                        if (!child.getName().equals("Caches")) {
                            moveAside(child, new File(new File(backup, "Library"), child.getName()));
                        }
                    }
                }
                step = getString(R.string.step_write_new_saves);
                File root = saveRoot(staging);
                int count = installSave(root, saveDir);
                message = getResources().getQuantityString(
                    R.plurals.imported_saves, count, name, count);
            } catch (Exception e) {
                message = getString(R.string.import_save_failed, step, e.getMessage(),
                    SAVE_BACKUPS_DIR);
            }
            deleteRecursively(staging);
            final String text = message;
            runOnUiThread(() -> Toast.makeText(this, text, Toast.LENGTH_LONG).show());
        }).start();
    }

    /// Moves `file` (if it exists) to `dest`, which must not exist yet.
    private void moveAside(File file, File dest) throws Exception {
        if (!file.exists()) {
            return;
        }
        dest.getParentFile().mkdirs();
        if (!file.renameTo(dest)) {
            throw new Exception(getString(R.string.cant_move_old_save, file.getName()));
        }
    }

    /// Where the saves are in an unpacked zip: possibly inside a single folder
    /// (e.g. a zipped `touchHLE_sandbox/<bundle id>` folder).
    private static File saveRoot(File staging) {
        File root = staging;
        while (true) {
            File[] children = root.listFiles();
            if (children == null) {
                return root;
            }
            File onlyDir = null;
            int others = 0;
            for (File child : children) {
                if (child.getName().equals(SAVE_MARKER)) {
                    continue;
                }
                if (child.isDirectory() && onlyDir == null) {
                    onlyDir = child;
                } else {
                    others++;
                }
            }
            if (onlyDir == null || others > 0 || onlyDir.getName().equals("Documents")
                || onlyDir.getName().equals("Library")) {
                return root;
            }
            root = onlyDir;
        }
    }

    /// Moves unpacked saves into the app's folder: `Documents` and `Library`
    /// keep their place, anything else goes into `Documents`.
    private static int installSave(File root, File saveDir) throws Exception {
        int count = 0;
        File[] children = root.listFiles();
        if (children == null) {
            return 0;
        }
        boolean structured = new File(root, "Documents").isDirectory()
            || new File(root, "Library").isDirectory();
        for (File child : children) {
            String childName = child.getName();
            if (childName.equals(SAVE_MARKER)) {
                continue;
            }
            File dest;
            if (structured && (childName.equals("Documents") || childName.equals("Library"))) {
                dest = new File(saveDir, childName);
            } else {
                dest = new File(new File(saveDir, "Documents"), childName);
            }
            count += moveTree(child, dest);
        }
        return count;
    }

    private static int moveTree(File source, File dest) throws Exception {
        if (source.isDirectory()) {
            dest.mkdirs();
            int count = 0;
            File[] children = source.listFiles();
            if (children != null) {
                for (File child : children) {
                    count += moveTree(child, new File(dest, child.getName()));
                }
            }
            return count;
        }
        dest.getParentFile().mkdirs();
        if (dest.exists()) {
            dest.delete();
        }
        if (!source.renameTo(dest)) {
            try (InputStream in = new FileInputStream(source);
                 OutputStream out = new FileOutputStream(dest)) {
                byte[] buffer = new byte[1 << 16];
                int n;
                while ((n = in.read(buffer)) != -1) {
                    out.write(buffer, 0, n);
                }
            }
        }
        return 1;
    }

    private void unzip(Uri uri, File dest) throws Exception {
        String destPath = dest.getCanonicalPath() + File.separator;
        int entries = 0;
        try (InputStream in = getContentResolver().openInputStream(uri);
             ZipInputStream zip = new ZipInputStream(in)) {
            ZipEntry entry;
            while ((entry = zip.getNextEntry()) != null) {
                entries++;
                String entryName = entry.getName().replace('\\', '/');
                // Skip files macOS adds to zips.
                if (entryName.startsWith("__MACOSX/") || entryName.endsWith("/.DS_Store")
                    || entryName.equals(".DS_Store")) {
                    continue;
                }
                File file = new File(dest, entryName);
                // Don't let a strange name escape the folder.
                if (!file.getCanonicalPath().startsWith(destPath)) {
                    continue;
                }
                if (entry.isDirectory()) {
                    file.mkdirs();
                    continue;
                }
                file.getParentFile().mkdirs();
                try (OutputStream out = new FileOutputStream(file)) {
                    byte[] buffer = new byte[1 << 16];
                    int n;
                    while ((n = zip.read(buffer)) != -1) {
                        out.write(buffer, 0, n);
                    }
                }
            }
        }
        if (entries == 0) {
            throw new Exception("Not a zip file");
        }
    }

    /// The bundle identifier in an exported save's marker file, if any.
    private static String readMarker(File marker) {
        byte[] data = readFile(marker, 4096);
        if (data == null) {
            return null;
        }
        for (String line : new String(data, StandardCharsets.UTF_8).split("\n")) {
            if (line.startsWith("bundle=")) {
                return line.substring("bundle=".length()).trim();
            }
        }
        return null;
    }

    // ---- Updates ----

    /// The version numbers in e.g. "v0.3.12" or "v0.3.12-2-gabcdef", or null.
    private static int[] parseVersion(String version) {
        Matcher m = Pattern.compile("v?(\\d+)\\.(\\d+)\\.(\\d+)").matcher(version);
        if (!m.find()) {
            return null;
        }
        return new int[]{Integer.parseInt(m.group(1)), Integer.parseInt(m.group(2)),
                         Integer.parseInt(m.group(3))};
    }

    private static boolean isNewer(int[] a, int[] b) {
        for (int i = 0; i < 3; i++) {
            if (a[i] != b[i]) {
                return a[i] > b[i];
            }
        }
        return false;
    }

    /// Asks GitHub for the latest release in the background, and shows a
    /// banner if it's newer than this version. Failures (e.g. no network) are
    /// ignored.
    private void checkForUpdate() {
        if (!getSharedPreferences(PREFS, MODE_PRIVATE).getBoolean(PREF_CHECK_UPDATES, true)) {
            return;
        }
        final int[] current = parseVersion(new String(nativeVersion(), StandardCharsets.UTF_8));
        if (current == null) {
            return;
        }
        new Thread(() -> {
            try {
                HttpURLConnection connection =
                    (HttpURLConnection) new URL(LATEST_RELEASE_URL).openConnection();
                connection.setConnectTimeout(8000);
                connection.setReadTimeout(8000);
                connection.setRequestProperty("Accept", "application/vnd.github+json");
                connection.setRequestProperty("User-Agent", "touchHLE-UF");
                byte[] data;
                try (InputStream in = connection.getInputStream()) {
                    java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream();
                    byte[] buffer = new byte[1 << 14];
                    int n;
                    while ((n = in.read(buffer)) != -1 && out.size() < (1 << 20)) {
                        out.write(buffer, 0, n);
                    }
                    data = out.toByteArray();
                } finally {
                    connection.disconnect();
                }
                JSONObject release = new JSONObject(new String(data, StandardCharsets.UTF_8));
                String tag = release.getString("tag_name");
                int[] latest = parseVersion(tag);
                if (latest == null || !isNewer(latest, current)) {
                    return;
                }
                final UpdateInfo info = new UpdateInfo();
                info.version = tag;
                info.notes = release.optString("body", "");
                info.downloadUrl = release.optString("html_url", "");
                JSONArray assets = release.optJSONArray("assets");
                for (int i = 0; assets != null && i < assets.length(); i++) {
                    JSONObject asset = assets.getJSONObject(i);
                    if (asset.optString("name").endsWith("_Android_AArch64.apk")) {
                        info.downloadUrl = asset.optString("browser_download_url", info.downloadUrl);
                    }
                }
                runOnUiThread(() -> {
                    update = info;
                    showUpdateBanner();
                });
            } catch (Exception e) {
                // No network, GitHub unreachable, etc.: just don't show anything.
            }
        }).start();
    }

    private void showUpdateBanner() {
        if (updateSlot == null) {
            return;
        }
        updateSlot.removeAllViews();
        String ignored = getSharedPreferences(PREFS, MODE_PRIVATE).getString(PREF_IGNORED_VERSION, "");
        if (update == null || update.version.equals(ignored)) {
            return;
        }
        TextView banner = makeText(getString(R.string.update_banner, update.version), 14, TEXT, true);
        banner.setGravity(Gravity.CENTER);
        banner.setPadding(dp(14), dp(10), dp(14), dp(10));
        banner.setBackground(buttonBackground(ACCENT, dp(12)));
        banner.setOnClickListener(v -> showUpdateDialog());
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(-1, -2);
        params.topMargin = dp(14);
        updateSlot.addView(banner, params);
    }

    /// The release notes are Markdown: keep the "Changes in ..." part (the
    /// rest describes the project and downloads), and drop the markup.
    private static String plainNotes(String notes) {
        notes = notes.replace("\r", "");
        int changes = notes.indexOf("Changes in ");
        if (changes >= 0) {
            notes = notes.substring(notes.lastIndexOf('\n', changes) + 1);
        }
        int built = notes.indexOf("\nBuilt from commit");
        if (built >= 0) {
            notes = notes.substring(0, built);
        }
        return notes.replaceAll("(?m)^#+\\s*", "")
            .replace("**", "")
            .replace("`", "")
            .replaceAll("\\[([^\\]]*)\\]\\([^)]*\\)", "$1")
            .trim();
    }

    private void showUpdateDialog() {
        final UpdateInfo info = update;
        if (info == null) {
            return;
        }
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle(getString(R.string.update_title, info.version))
            .setMessage(plainNotes(info.notes))
            .setPositiveButton(R.string.download_apk, (dialog, which) -> {
                try {
                    startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(info.downloadUrl)));
                } catch (ActivityNotFoundException e) {
                    Toast.makeText(this, R.string.no_browser, Toast.LENGTH_LONG).show();
                }
            })
            .setNegativeButton(R.string.later, null)
            .setNeutralButton(R.string.ignore_version, (dialog, which) -> {
                getSharedPreferences(PREFS, MODE_PRIVATE).edit()
                    .putString(PREF_IGNORED_VERSION, info.version).apply();
                showUpdateBanner();
            })
            .show();
    }

    /// A gear icon for the settings button, drawn here rather than taken from
    /// an icon set.
    private static class GearDrawable extends Drawable {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);

        GearDrawable(int color) {
            paint.setColor(color);
            paint.setStyle(Paint.Style.FILL);
        }

        @Override
        public void draw(Canvas canvas) {
            Rect bounds = getBounds();
            float cx = bounds.exactCenterX();
            float cy = bounds.exactCenterY();
            float outer = Math.min(bounds.width(), bounds.height()) / 2f;
            float inner = outer * 0.74f;
            int teeth = 8;
            Path path = new Path();
            path.setFillType(Path.FillType.EVEN_ODD);
            for (int i = 0; i < teeth; i++) {
                double center = 2 * Math.PI * i / teeth;
                double[] angles = {center - 0.30, center - 0.18, center + 0.18, center + 0.30};
                float[] radii = {inner, outer, outer, inner};
                for (int j = 0; j < 4; j++) {
                    float x = cx + (float) Math.cos(angles[j]) * radii[j];
                    float y = cy + (float) Math.sin(angles[j]) * radii[j];
                    if (i == 0 && j == 0) {
                        path.moveTo(x, y);
                    } else {
                        path.lineTo(x, y);
                    }
                }
            }
            path.close();
            path.addCircle(cx, cy, outer * 0.32f, Path.Direction.CW);
            canvas.drawPath(path, paint);
        }

        @Override
        public void setAlpha(int alpha) {
            paint.setAlpha(alpha);
        }

        @Override
        public void setColorFilter(ColorFilter filter) {
            paint.setColorFilter(filter);
        }

        @Override
        public int getOpacity() {
            return PixelFormat.TRANSLUCENT;
        }

        @Override
        public int getIntrinsicWidth() {
            return 48;
        }

        @Override
        public int getIntrinsicHeight() {
            return 48;
        }
    }
}
