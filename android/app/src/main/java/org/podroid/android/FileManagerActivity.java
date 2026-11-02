/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.podroid.android;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.provider.DocumentsContract;
import android.widget.Toast;

/**
 * Opens a file manager showing touchHLE's files (see DocumentsProvider).
 *
 * touchHLE requests this by opening a "&lt;applicationId&gt;.files://" URL (see
 * src/paths.rs). Simply viewing the provider's root URI, as was done before,
 * doesn't work on many devices whose vendor replaced the system file manager,
 * so several ways are tried in turn.
 */
public class FileManagerActivity extends Activity {
    // DocumentsContract.Root.MIME_TYPE_ITEM
    private static final String ROOT_MIME_TYPE = "vnd.android.document/root";

    @Override
    protected void attachBaseContext(Context base) {
        super.attachBaseContext(Languages.wrap(base));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        Uri rootUri = DocumentsContract.buildRootUri(
            getPackageName() + ".provider", DocumentsProvider.ROOT_ID);

        Intent[] attempts = {
            // The standard way: the system's documents UI handles this type.
            viewIntent(rootUri, ROOT_MIME_TYPE, null),
            // The documents UI explicitly, in case another app grabbed the
            // generic intent or the type isn't matched. Many vendor ROMs keep
            // it installed even though it has no launcher icon.
            viewIntent(rootUri, ROOT_MIME_TYPE, "com.google.android.documentsui"),
            viewIntent(rootUri, ROOT_MIME_TYPE, "com.android.documentsui"),
            // What touchHLE did originally.
            viewIntent(rootUri, null, null),
        };

        boolean opened = false;
        for (Intent intent : attempts) {
            try {
                startActivity(intent);
                opened = true;
                break;
            } catch (ActivityNotFoundException | SecurityException e) {
                // Try the next one.
            }
        }
        if (!opened) {
            Toast.makeText(
                this,
                getString(R.string.file_manager_failed, getString(R.string.app_name)),
                Toast.LENGTH_LONG
            ).show();
        }
        finish();
    }

    private static Intent viewIntent(Uri uri, String type, String packageName) {
        Intent intent = new Intent(Intent.ACTION_VIEW);
        if (type != null) {
            intent.setDataAndType(uri, type);
        } else {
            intent.setData(uri);
        }
        if (packageName != null) {
            intent.setPackage(packageName);
        }
        intent.addCategory(Intent.CATEGORY_DEFAULT);
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
            | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            | Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
        return intent;
    }
}
