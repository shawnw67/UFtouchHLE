/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * Parts of this file are derived from SDL 2's Android project template, which
 * has a different license. Please see vendor/SDL/LICENSE.txt for details.
 */
import org.gradle.nativeplatform.platform.internal.DefaultNativePlatform

plugins {
    id("com.android.application") version("8.10.1")
    id("com.github.willir.rust.cargo-ndk-android") version("0.3.4")
    id("org.jetbrains.kotlin.android") version("2.0.21")
}

fun runTouchHLEVersionTool(wantBranding: Boolean): String {
    val output = providers.exec {
        commandLine("cargo", "run", "--package", "Podroid_version")
        if (wantBranding) {
            args("--", "--branding")
        }
    }.standardOutput.asText.get().trim()

    return output
}

fun getTouchHLEBranding(): String {
    return runTouchHLEVersionTool(/* wantBranding: */ true)
}

fun getTouchHLEVersionName(): String {
    return runTouchHLEVersionTool(/* wantBranding: */ false)
}

fun join(prefix: String, separator: String, branding: String): String {
    return if (branding.isEmpty()) prefix else prefix + separator + branding
}

android {
    // NDK r28+ links 64-bit ELF LOAD segments aligned to 16 KB by default,
    // which is required on Android devices with a 16 KB page size. Older NDKs
    // (e.g. r25) only produce 4 KB aligned libraries, which the OS refuses to
    // load. 16 KB alignment for libPodroid.so/libSDL2.so is additionally
    // enforced via the rustflags in .cargo/config.toml.
    ndkVersion = "28.2.13676358"
    compileSdk = 36
    buildFeatures {
        buildConfig = true
    }
    defaultConfig {
        val branding = getTouchHLEBranding()
        // Must match src/paths.rs.
        if (branding == "UF") {
            applicationId = "org.podroid.android"
        } else {
            applicationId = "org.podroid.android"
            if (!branding.isEmpty()) {
                applicationIdSuffix = branding.lowercase()
            }
        }
        resValue("string", "app_name", "Podroid")
        buildConfigField("String", "APP_NAME", "\"Podroid\"")
        val icon = if (branding == "UF") "@mipmap/ic_launcher"
            else join("@drawable/icon", "_", branding.lowercase())
        manifestPlaceholders["icon"] = icon
        manifestPlaceholders["roundIcon"] =
            if (branding == "UF") "@mipmap/ic_launcher_round" else icon
        buildConfigField("int", "APP_ICON", if (branding == "UF") "R.mipmap.ic_launcher"
            else join("R.drawable.icon", "_", branding.lowercase()))
        versionName = getTouchHLEVersionName()

        minSdk = 29 // Android 10
        targetSdk = 36
        externalNativeBuild {
            ndkBuild {
                arguments("APP_PLATFORM=android-29")
                // abiFilters 'armeabi-v7a', 'arm64-v8a', 'x86', 'x86_64'
                // Only 'arm64-v8a' and 'x86_64' are supported by dynarmic
                // and hence touchHLE. The 'x86_64' build works, but the main
                // use for that would be the emulator in Android Studio, and
                // its OpenGL ES implementations don't seem to work properly
                // with touchHLE, so we disable it to reduce build time and
                // avoid shipping stuff we haven't meaningfully tested.
                // Make sure this matches the cargoNdk targets below.
                abiFilters("arm64-v8a")
            }
        }
    }
    // The target JVM version must be the same for Java and Kotlin.
    compileOptions {
        sourceCompatibility(JavaVersion.VERSION_11)
        targetCompatibility(JavaVersion.VERSION_11)
    }
    kotlinOptions {
        jvmTarget = "11"
    }
    // Release builds are signed with a fixed key when one is provided (see the
    // GitHub Actions workflow), so newer APKs can be installed over older ones.
    // Otherwise the debug key is used, which differs between machines.
    val releaseKeystore = System.getenv("TOUCHHLE_KEYSTORE_FILE")
    signingConfigs {
        if (!releaseKeystore.isNullOrEmpty()) {
            create("release") {
                storeFile = file(releaseKeystore)
                storePassword = System.getenv("TOUCHHLE_KEYSTORE_PASSWORD")
                keyAlias = "touchhle-uf"
                keyPassword = System.getenv("TOUCHHLE_KEYSTORE_PASSWORD")
            }
        }
    }
    buildTypes {
        release {
            signingConfig = if (!releaseKeystore.isNullOrEmpty()) {
                signingConfigs.getByName("release")
            } else {
                signingConfigs.getByName("debug")
            }
            isMinifyEnabled = false
            // Release builds must NOT be debuggable: a debuggable app is
            // treated by Android as debuggable (allows adb, exposes debug
            // surfaces, and is subject to stricter restrictions). AGP's
            // default for release is isDebuggable=false, so we simply don't
            // set it here.
        }
        debug {
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("**/*.so")
            }
            isDebuggable = true
            isJniDebuggable = true
        }
    }

    applicationVariants.all {
        val variantName = name.replaceFirstChar { char ->
            if (char.isLowerCase()) char.titlecase() else char.toString()
        }
        tasks.named("merge${variantName}Assets").configure {
            dependsOn("externalNativeBuild${variantName}")
        }
        // The Rust library is copied into jniLibs by the cargo-ndk plugin, so
        // it has to be built before the native libraries are merged, or the
        // APK can get the library from the previous build.
        tasks.named("merge${variantName}JniLibFolders").configure {
            dependsOn("buildCargoNdk${variantName}")
        }
        // Patch the SDL2 source that the bundled rust-sdl2 build compiles (it
        // comes from the cargo checkout of rust-sdl2, not vendor/SDL) before
        // cargo-ndk builds, so the NDK r28+ obsoleted ALooper_pollAll call is
        // replaced with ALooper_pollOnce.
        tasks.named("buildCargoNdk${variantName}").configure {
            dependsOn(patchSdlForNdk28)
        }
    }

    sourceSets {
        getByName("main") {
            java.srcDir("${rootDir.parentFile}/vendor/SDL/android-project/app/src/main/java")
        }
    }

    if (!project.hasProperty("EXCLUDE_NATIVE_LIBS")) {
        sourceSets {
            getByName("main") {
                jniLibs.srcDir("${projectDir}/jniLibs")
            }
        }
        externalNativeBuild {
            ndkBuild {
                path("jni/Android.mk")
            }
        }
    }

    lint {
        abortOnError = false
    }
    namespace = "org.podroid.android"
}

cargoNdk {
    // Make sure this matches the android abiFilters above.
    targets = arrayListOf("arm64")
    // Must match minSdk above.
    apiLevel = 29
    module = ".."
    librariesNames = arrayListOf("libPodroid.so", "libSDL2.so", "libc++_shared.so")
    extraCargoEnv = mapOf(
        "ANDROID_NDK" to android.ndkDirectory.toString(),
        "ANDROID_NDK_HOME" to android.ndkDirectory.toString(),
        // NDK r28 marks some APIs as strict-availability errors for the app's
        // minSdk. Defining __ANDROID_UNAVAILABLE_SYMBOLS_ARE_WEAK__ (see NDK's
        // android/versioning.h) turns those into weak references, keeping
        // SDL2 2.26 buildable. (The separately-patched ALooper_pollAll call,
        // which uses the obsoleted= form that this flag does not affect, is
        // handled by dev-scripts/patch-sdl-ndk28.sh.)
        "CFLAGS_aarch64_linux_android" to "-D__ANDROID_UNAVAILABLE_SYMBOLS_ARE_WEAK__",
    )

    if (DefaultNativePlatform.host().operatingSystem.isWindows) {
        val binPath =
            android.ndkDirectory.toPath().resolve("toolchains/llvm/prebuilt/windows-x86_64/bin")
        val clangPath = binPath.resolve("clang.exe")
        val clangXXPath = binPath.resolve("clang++.exe")

        if (!clangPath.toFile().exists()) {
            throw GradleException("NDK clang compiler not found at expected location: $clangPath")
        }
        if (!clangXXPath.toFile().exists()) {
            throw GradleException("NDK clang++ compiler not found at expected location: $clangXXPath")
        }

        extraCargoEnv.putAll(
            mapOf(
                "CC" to clangPath.toString(),
                "CXX" to clangXXPath.toString(),
                // The default generator on Windows (Visual Studio) does not respect
                // the CC and CXX environment variables. Using Ninja ensures that
                // the specified compilers are used
                "CMAKE_GENERATOR" to "Ninja",
            )
        )
    }
    // The default feature, "static", makes us use static linking for SDL2 and OpenAL Soft.
    // For Android, we need dynamic linking for SDL2, but static linking for OpenAL Soft.
    extraCargoBuildArguments = arrayListOf(
        "--lib",
        "--no-default-features",
        "--features",
        "Podroid_openal_soft_wrapper/static,sdl2/bundled"
    )
}

// The bundled rust-sdl2 build compiles SDL 2.26 from the cargo checkout of the
// rust-sdl2 dependency (sdl2-sys/SDL). That SDL calls ALooper_pollAll(), which
// NDK r28+ declares with __REMOVED_IN(1,...) and therefore rejects as a hard
// compile error. Patch the call site to ALooper_pollOnce (same signature, and
// the replacement used by upstream SDL 2.28+) before cargo-ndk builds, so local
// and CI builds are consistent. The script is idempotent (cargo re-checks out
// the dependency on a clean checkout, e.g. in CI).
val patchSdlForNdk28 by tasks.registering(Exec::class) {
    val script = rootDir.parentFile.resolve("dev-scripts/patch-sdl-ndk28.sh")
    commandLine("bash", script.absolutePath)
}

dependencies {
    implementation(fileTree("libs") {
        include("*.jar")
    })
}
