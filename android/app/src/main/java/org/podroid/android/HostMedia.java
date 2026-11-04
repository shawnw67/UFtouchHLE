/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.podroid.android;

import android.Manifest;
import android.annotation.SuppressLint;
import android.content.pm.PackageManager;
import android.graphics.ImageFormat;
import android.hardware.camera2.CameraCaptureSession;
import android.hardware.camera2.CameraCharacteristics;
import android.hardware.camera2.CameraDevice;
import android.hardware.camera2.CameraManager;
import android.hardware.camera2.CaptureRequest;
import android.media.AudioFormat;
import android.media.AudioRecord;
import android.media.Image;
import android.media.ImageReader;
import android.media.MediaRecorder;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.util.Log;

import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import java.nio.ShortBuffer;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;


/**
 * Real host camera & microphone access for the emulated iOS apps.
 *
 * Called from Rust (src/android_media.rs) via JNI static methods. Every
 * method degrades gracefully: if the permission is missing, the hardware is
 * absent, or anything throws, the callers get "not available" results instead
 * of crashes, so emulated apps see an honest "no camera / no mic" device.
 */
public class HostMedia {
    private static final String TAG = "HostMedia";

    /** Most recent still photo (JPEG bytes), filled by takePhoto(). */
    private static byte[] lastPhoto = null;

    // ------------------------------------------------------------------
    // Microphone state
    // ------------------------------------------------------------------
    private static AudioRecord micRecord = null;
    private static Thread micThread = null;
    private static volatile boolean micRunning = false;
    /** Latest mic chunk: mono 16-bit LE PCM at MIC_SAMPLE_RATE. */
    private static volatile short[] micLatest = new short[0];

    public static final int MIC_SAMPLE_RATE = 44100;
    private static final int MIC_CHUNK_FRAMES = 1024;

    // ------------------------------------------------------------------
    // Permissions (blocking on the SDL thread, resolved on the UI thread)
    // ------------------------------------------------------------------

    private static boolean hasPermission(String permission) {
        android.app.Activity act = MainActivity.getActivity();
        if (act == null) {
            Log.w(TAG, "hasPermission(" + permission + "): no Activity");
            return false;
        }
        // Activity.checkSelfPermission() is API 23+ and minSdkVersion is 21.
        if (android.os.Build.VERSION.SDK_INT < 23) {
            return act.checkPermission(permission,
                    android.os.Process.myPid(),
                    android.os.Process.myUid()) == PackageManager.PERMISSION_GRANTED;
        }
        return act.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED;
    }

    /** Request permissions and block until answered (or timeout). */
    private static boolean ensurePermissions(String[] permissions) {
        android.app.Activity act = MainActivity.getActivity();
        if (act == null) {
            return false;
        }
        boolean missing = false;
        for (String p : permissions) {
            if (!hasPermission(p)) {
                missing = true;
                break;
            }
        }
        if (!missing) {
            return true;
        }
        final AtomicReference<Boolean> granted = new AtomicReference<Boolean>(Boolean.FALSE);
        try {
            // Activity.requestPermissions must run on the UI thread; SDL
            // calls arrive on the native thread, so hop over first.
            act.runOnUiThread(new Runnable() {
                public void run() {
                    act.requestPermissions(permissions, 0x484D); // "HM"
                }
            });
            // Poll until the dialog is answered  -  the system updates the
            // grant state regardless of the callback, so polling
            // checkSelfPermission for up to 15 seconds is robust.
            for (int i = 0; i < 300; i++) {
                Thread.sleep(50);
                boolean all = true;
                for (String p : permissions) {
                    if (!hasPermission(p)) {
                        all = false;
                        break;
                    }
                }
                if (all) {
                    granted.set(Boolean.TRUE);
                    break;
                }
            }
        } catch (Throwable t) {
            Log.e(TAG, "ensurePermissions failed", t);
        }
        return granted.get();
    }

    // ------------------------------------------------------------------
    // Camera
    // ------------------------------------------------------------------

    /** Whether a camera with the given facing exists AND we can use it. */
    @SuppressLint("MissingPermission")
    public static boolean hasCamera(final boolean front) {
        final String which = front ? "front" : "rear";
        android.app.Activity act = MainActivity.getActivity();
        if (act == null) {
            Log.w(TAG, "hasCamera(" + which + "): no Activity");
            return false;
        }
        if (android.os.Build.VERSION.SDK_INT < 21) {
            Log.w(TAG, "hasCamera(" + which + "): Camera2 needs API 21+");
            return false;
        }
        try {
            // Pure hardware probe  -  NO permission request here. Games poll
            // availability during startup; the CAMERA permission dialog is
            // shown by takePhoto() when capture actually starts.
            CameraManager cm =
                    (CameraManager) act.getSystemService(android.content.Context.CAMERA_SERVICE);
            if (cm == null) {
                Log.w(TAG, "hasCamera(" + which + "): no CameraManager");
                return cameraFeature(act, front);
            }
            int seen = 0;
            for (String id : cm.getCameraIdList()) {
                seen++;
                CameraCharacteristics cc = cm.getCameraCharacteristics(id);
                Integer facing = cc.get(CameraCharacteristics.LENS_FACING);
                if (facing == null) {
                    continue;
                }
                boolean isFront = facing == CameraCharacteristics.LENS_FACING_FRONT;
                if (isFront == front) {
                    return true;
                }
            }
            Log.w(TAG, "hasCamera(" + which + "): none of the " + seen
                    + " host cameras faces that way");
        } catch (Throwable t) {
            // A Camera2 service that throws must not read as "this device has
            // no camera": fall back to the system feature flag.
            Log.e(TAG, "hasCamera(" + which + ") threw, using the feature flag", t);
            return cameraFeature(act, front);
        }
        return false;
    }

    /** System feature fallback for when Camera2 cannot answer at all. */
    private static boolean cameraFeature(android.content.Context ctx, boolean front) {
        String feature = front ? PackageManager.FEATURE_CAMERA_FRONT
                : PackageManager.FEATURE_CAMERA;
        boolean has = ctx.getPackageManager().hasSystemFeature(feature);
        Log.w(TAG, "cameraFeature(" + (front ? "front" : "rear") + ") = " + has);
        return has;
    }

    /**
     * Capture a still photo from the host camera and return JPEG bytes, or
     * null if unavailable. Blocks up to ~8 seconds.
     */
    @SuppressLint("MissingPermission")
    public static byte[] takePhoto(final boolean front) {
        final String which = front ? "front" : "rear";
        android.app.Activity act = MainActivity.getActivity();
        if (act == null || android.os.Build.VERSION.SDK_INT < 21) {
            Log.w(TAG, "takePhoto(" + which + "): no Activity or API < 21");
            return null;
        }
        try {
            if (!ensurePermissions(new String[] { Manifest.permission.CAMERA })) {
                Log.w(TAG, "takePhoto(" + which + "): CAMERA permission denied");
                return null;
            }
            CameraManager cm =
                    (CameraManager) act.getSystemService(android.content.Context.CAMERA_SERVICE);
            if (cm == null) {
                return null;
            }
            String cameraId = null;
            for (String id : cm.getCameraIdList()) {
                CameraCharacteristics cc = cm.getCameraCharacteristics(id);
                Integer facing = cc.get(CameraCharacteristics.LENS_FACING);
                if (facing == null) {
                    continue;
                }
                boolean isFront = facing == CameraCharacteristics.LENS_FACING_FRONT;
                if (isFront == front) {
                    cameraId = id;
                    break;
                }
            }
            if (cameraId == null) {
                Log.w(TAG, "takePhoto(" + which + "): no camera with that facing");
                return null;
            }
            final CountDownLatch done = new CountDownLatch(1);
            final AtomicReference<byte[]> photo = new AtomicReference<byte[]>(null);
            HandlerThread ht = new HandlerThread("HyperHLECamera");
            ht.start();
            final Handler handler = new Handler(ht.getLooper());

            final ImageReader reader =
                    ImageReader.newInstance(1280, 720, ImageFormat.JPEG, 2);
            reader.setOnImageAvailableListener(new ImageReader.OnImageAvailableListener() {
                @Override
                public void onImageAvailable(ImageReader r) {
                    Image image = null;
                    try {
                        image = r.acquireLatestImage();
                        if (image != null) {
                            ByteBuffer buf = image.getPlanes()[0].getBuffer();
                            byte[] data = new byte[buf.remaining()];
                            buf.get(data);
                            photo.set(data);
                        }
                    } catch (Throwable t) {
                        Log.e(TAG, "photo read failed", t);
                    } finally {
                        if (image != null) {
                            image.close();
                        }
                        done.countDown();
                    }
                }
            }, handler);

            final AtomicReference<CameraDevice> camRef =
                    new AtomicReference<CameraDevice>(null);
            cm.openCamera(cameraId, new CameraDevice.StateCallback() {
                @Override
                public void onOpened(CameraDevice camera) {
                    camRef.set(camera);
                    try {
                    } catch (Throwable ignored) {
                    }
                    try {
                        CameraCaptureSession.StateCallback cb =
                                new CameraCaptureSession.StateCallback() {
                                    @Override
                                    public void onConfigured(
                                            CameraCaptureSession session) {
                                        try {
                                            CaptureRequest.Builder req = camera
                                                    .createCaptureRequest(
                                                            CameraDevice.TEMPLATE_STILL_CAPTURE);
                                            req.addTarget(reader.getSurface());
                                            req.set(CaptureRequest
                                                            .CONTROL_AF_MODE,
                                                    CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE);
                                            session.capture(req.build(), null, null);
                                        } catch (Throwable t) {
                                            Log.e(TAG, "capture failed", t);
                                            done.countDown();
                                        }
                                    }

                                    @Override
                                    public void onConfigureFailed(
                                            CameraCaptureSession session) {
                                        Log.e(TAG, "camera configure failed");
                                        done.countDown();
                                    }
                                };
                        camera.createCaptureSession(
                                java.util.Collections.singletonList(reader.getSurface()),
                                cb, handler);
                    } catch (Throwable t) {
                        Log.e(TAG, "createCaptureSession failed", t);
                        done.countDown();
                    }
                }

                @Override
                public void onDisconnected(CameraDevice camera) {
                    try {
                        camera.close();
                    } catch (Throwable ignored) {
                    }
                    done.countDown();
                }

                @Override
                public void onError(CameraDevice camera, int error) {
                    Log.e(TAG, "camera open error " + error);
                    try {
                        camera.close();
                    } catch (Throwable ignored) {
                    }
                    done.countDown();
                }
            }, handler);

            done.await(8, TimeUnit.SECONDS);
            try {
                CameraDevice cam = camRef.get();
                if (cam != null) {
                    cam.close();
                }
            } catch (Throwable ignored) {
            }
            ht.quitSafely();
            byte[] result = photo.get();
            if (result == null) {
                Log.w(TAG, "takePhoto(" + which + "): no frame within the timeout");
            } else {
                Log.i(TAG, "takePhoto(" + which + "): " + result.length + " JPEG bytes");
            }
            lastPhoto = result;
            return result;
        } catch (Throwable t) {
            Log.e(TAG, "takePhoto failed", t);
            return null;
        }
    }

    // ------------------------------------------------------------------
    // Microphone
    // ------------------------------------------------------------------

    /** Whether the host has a usable microphone. */
    public static boolean hasMicrophone() {
        android.app.Activity act = MainActivity.getActivity();
        if (act == null) {
            Log.w(TAG, "hasMicrophone: no Activity");
            return false;
        }
        try {
            // Pure hardware probe  -  NO permission request here. Games check
            // isInputAvailable during startup; the RECORD_AUDIO permission
            // dialog is shown by startMic() when capture actually starts.
            if (!act.getPackageManager().hasSystemFeature(
                    android.content.pm.PackageManager.FEATURE_MICROPHONE)) {
                Log.w(TAG, "hasMicrophone: no FEATURE_MICROPHONE");
                return false;
            }
            int min =
                    AudioRecord.getMinBufferSize(MIC_SAMPLE_RATE,
                            AudioFormat.CHANNEL_IN_MONO,
                            AudioFormat.ENCODING_PCM_16BIT);
            if (min <= 0) {
                Log.w(TAG, "hasMicrophone: getMinBufferSize() = " + min);
            }
            return min > 0;
        } catch (Throwable t) {
            Log.e(TAG, "hasMicrophone failed", t);
            return false;
        }
    }

    /** Start streaming mic PCM. Returns false when no mic is usable. */
    @SuppressLint("MissingPermission")
    public static boolean startMic() {
        if (micRunning) {
            return true;
        }
        // The AudioRecord constructor parcels an AttributionSource with
        // android.os.Parcel native (@FastNative) writes. Doing that while the
        // calling (SDL) thread is inside a nested native-to-Java transition
        // trips CheckJNI's "invalid JNI transition frame reference" abort on
        // Android 14+. Creating the AudioRecord on the Android main thread (a
        // plain Java thread) keeps all JNI transitions well-formed.
        final AtomicReference<AudioRecord> built =
                new AtomicReference<AudioRecord>(null);
        final CountDownLatch done = new CountDownLatch(1);
        new Handler(Looper.getMainLooper()).post(new Runnable() {
            public void run() {
                try {
                    built.set(buildMicRecord());
                } catch (Throwable t) {
                    Log.e(TAG, "startMic failed", t);
                } finally {
                    done.countDown();
                }
            }
        });
        try {
            // The guest blocks in AudioQueueStart anyway; a slow main thread
            // (e.g. a permission dialog) must not wedge the emulator forever.
            if (!done.await(15, TimeUnit.SECONDS)) {
                Log.w(TAG, "startMic: timed out waiting for the main thread");
                return false;
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            return false;
        }
        AudioRecord record = built.get();
        if (record == null) {
            return false;
        }
        micRecord = record;
        micLatest = new short[0];
        micRunning = true;
        record.startRecording();
        micThread = new Thread(new Runnable() {
            public void run() {
                short[] chunk = new short[MIC_CHUNK_FRAMES];
                while (micRunning) {
                    int n = 0;
                    try {
                        n = record.read(chunk, 0, chunk.length);
                    } catch (Throwable t) {
                        Log.e(TAG, "mic read failed", t);
                        break;
                    }
                    if (n > 0) {
                        short[] out = new short[n];
                        System.arraycopy(chunk, 0, out, 0, n);
                        micLatest = out;
                    } else {
                        micLatest = new short[0];
                    }
                }
            }
        }, "HyperHLE-Mic");
        micThread.start();
        return true;
    }

    /** Runs on the Android main thread; returns a started-ready AudioRecord. */
    @SuppressLint("MissingPermission")
    private static AudioRecord buildMicRecord() {
        android.app.Activity act = MainActivity.getActivity();
        if (act == null) {
            Log.w(TAG, "startMic: no Activity");
            return null;
        }
        try {
            if (!act.getPackageManager().hasSystemFeature(
                    android.content.pm.PackageManager.FEATURE_MICROPHONE)) {
                Log.w(TAG, "startMic: no FEATURE_MICROPHONE");
                return null;
            }
            // Capture is actually starting now  -  this is where the
            // RECORD_AUDIO permission dialog belongs. Without it AudioRecord
            // never reaches STATE_INITIALIZED on Android 6+.
            if (!ensurePermissions(new String[] { Manifest.permission.RECORD_AUDIO })) {
                Log.w(TAG, "startMic: RECORD_AUDIO permission denied");
                return null;
            }
            int min =
                    AudioRecord.getMinBufferSize(MIC_SAMPLE_RATE,
                            AudioFormat.CHANNEL_IN_MONO,
                            AudioFormat.ENCODING_PCM_16BIT);
            if (min <= 0) {
                Log.w(TAG, "startMic: getMinBufferSize() = " + min);
                return null;
            }
            int bufferSize = Math.max(min, MIC_CHUNK_FRAMES * 4);
            final AudioRecord record =
                    new AudioRecord(MediaRecorder.AudioSource.MIC, MIC_SAMPLE_RATE,
                            AudioFormat.CHANNEL_IN_MONO,
                            AudioFormat.ENCODING_PCM_16BIT, bufferSize);
            if (record.getState() != AudioRecord.STATE_INITIALIZED) {
                Log.w(TAG, "startMic: AudioRecord state = " + record.getState()
                        + " (not STATE_INITIALIZED)");
                record.release();
                return null;
            }
            return record;
        } catch (Throwable t) {
            Log.e(TAG, "startMic failed", t);
            return null;
        }
    }

    public static void stopMic() {
        micRunning = false;
        if (micThread != null) {
            micThread.interrupt();
        }
        micThread = null;
        micLatest = new short[0];
        if (micRecord != null) {
            try {
                micRecord.stop();
            } catch (Throwable ignored) {
            }
            try {
                micRecord.release();
            } catch (Throwable ignored) {
            }
            micRecord = null;
        }
    }

    /**
     * One-line report of what this device/permission state currently allows.
     * Rust calls it once at startup (android_media::populate_jni_cache) and
     * logs the result, so a "no camera / no microphone" answer can be
     * attributed from the touchHLE log alone.
     */
    public static String diagnose() {
        StringBuilder sb = new StringBuilder();
        android.app.Activity act = MainActivity.getActivity();
        sb.append("activity=").append(act == null ? "NULL" : "ok");
        sb.append(" sdk=").append(android.os.Build.VERSION.SDK_INT);
        sb.append(" rear=").append(hasCamera(false));
        sb.append(" front=").append(hasCamera(true));
        sb.append(" mic=").append(hasMicrophone());
        sb.append(" cameraPerm=")
                .append(hasPermission(Manifest.permission.CAMERA));
        sb.append(" micPerm=")
                .append(hasPermission(Manifest.permission.RECORD_AUDIO));
        sb.append(" micRunning=").append(micRunning);
        return sb.toString();
    }

    /**
     * @return the most recent mic chunk (mono 16-bit LE PCM), or an empty
     *         array when nothing new was captured. Rust converts to the
     *         guest's requested stream format.
     */
    public static short[] readMicChunk() {
        short[] chunk = micLatest;
        if (chunk == null) {
            return new short[0];
        }
        short[] copy = new short[chunk.length];
        System.arraycopy(chunk, 0, copy, 0, chunk.length);
        return copy;
    }
}
