/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.podroid.android;

import android.content.Context;
import android.content.res.Configuration;
import android.os.LocaleList;

import java.util.Locale;

/**
 * The language of touchHLE's own Android UI (the launcher, its dialogs and
 * notifications). It follows the system unless one is picked in the
 * launcher's settings. Each activity and service applies it with
 * {@link #wrap} in attachBaseContext().
 *
 * The choice is kept in the launcher's preferences. MainActivity runs in its
 * own process, but it starts after the choice is made, so it reads the
 * current value.
 */
final class Languages {
    private Languages() {}

    private static final String PREFS = "launcher";
    private static final String PREF_LANGUAGE = "language";

    /// BCP 47 tags of the translations, in the order they're offered. The
    /// empty tag means following the system.
    static final String[] TAGS = {"", "zh-Hans", "zh-Hant", "en", "ja"};
    /// Each language's name in itself, so it can be found whatever the
    /// current language is. The first entry is replaced by a translation of
    /// "System default".
    private static final String[] NAMES = {"", "简体中文", "繁體中文", "English", "日本語"};

    static String chosenTag(Context context) {
        String tag = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString(PREF_LANGUAGE, "");
        for (String known : TAGS) {
            if (known.equals(tag)) {
                return tag;
            }
        }
        return "";
    }

    static void choose(Context context, String tag) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString(PREF_LANGUAGE, tag).commit();
    }

    /** The names to offer, with the system default named in the UI's language. */
    static String[] names(Context context) {
        String[] names = NAMES.clone();
        names[0] = context.getString(R.string.language_system);
        return names;
    }

    static String name(Context context, String tag) {
        for (int i = 0; i < TAGS.length; i++) {
            if (TAGS[i].equals(tag)) {
                return names(context)[i];
            }
        }
        return names(context)[0];
    }

    /** A context using the chosen language, or {@code base} if there is none. */
    static Context wrap(Context base) {
        String tag = chosenTag(base);
        if (tag.isEmpty()) {
            return base;
        }
        Configuration config = new Configuration(base.getResources().getConfiguration());
        Locale locale = withRegion(Locale.forLanguageTag(tag), systemRegion(config));
        config.setLocales(new LocaleList(locale));
        return base.createConfigurationContext(config);
    }

    private static String systemRegion(Configuration config) {
        Locale locale = config.getLocales().get(0);
        return locale != null ? locale.getCountry() : "";
    }

    /**
     * The chosen language with the system's region, which games get as the
     * current locale's country (e.g. for dates or which server to use).
     *
     * touchHLE tells Simplified and Traditional Chinese apart by the region
     * (see src/window.rs), so they get a region that matches if the system's
     * doesn't.
     */
    private static Locale withRegion(Locale language, String region) {
        boolean traditionalRegion = region.equals("TW") || region.equals("HK")
            || region.equals("MO");
        if (language.getScript().equals("Hant") && !traditionalRegion) {
            region = "TW";
        } else if (language.getScript().equals("Hans") && traditionalRegion) {
            region = "CN";
        }
        if (region.isEmpty()) {
            return language;
        }
        return new Locale.Builder().setLocale(language).setRegion(region).build();
    }

    /**
     * The languages to prefer for apps' names, as language codes separated by
     * commas: the chosen one first, then the system's.
     */
    static String preferred(Context context) {
        StringBuilder result = new StringBuilder();
        String tag = chosenTag(context);
        if (!tag.isEmpty()) {
            result.append(Locale.forLanguageTag(tag).getLanguage());
        }
        LocaleList locales = LocaleList.getDefault();
        for (int i = 0; i < locales.size(); i++) {
            appendLanguage(result, locales.get(i).getLanguage());
        }
        return result.toString();
    }

    private static void appendLanguage(StringBuilder list, String language) {
        if (list.length() > 0) {
            list.append(',');
        }
        list.append(language);
    }
}
