/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Logging and terminal output macros.

use std::fs::File;
use std::sync::LazyLock;

/// Whether to write to the log file. Turned off by the Android launcher
/// (see src/android_launcher.rs), which runs in a different process from the
/// app and would otherwise replace the app's log.
pub static LOG_TO_FILE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Get a handle to the log file. This is only for use by logging macros!
///
/// All the logging macros print to stderr or (on Android) logcat, but this
/// is not convenient for users who aren't accustomed to command-line tools or
/// who don't have access to ADB, so we also write to a log file.
pub fn get_log_file() -> &'static File {
    static LOG_FILE: LazyLock<File> = LazyLock::new(|| {
        let base = crate::paths::user_data_base_path();
        let path = base.join("touchHLE_log.txt");
        // Keep the log of the previous run. If touchHLE crashed (especially
        // on Android, where it's easy to relaunch it straight away), that log
        // is the one that's needed to find out why.
        let _ = std::fs::rename(&path, base.join("touchHLE_log_previous.txt"));
        File::create(path).unwrap()
    });

    &LOG_FILE
}

/// Prints a log message unconditionally. Use this for errors or warnings.
///
/// The message is prefixed with the module path, so it is clear where it comes
/// from.
macro_rules! log {
    ($($arg:tt)+) => {
        {
            echo!("{}: {}", module_path!(), format_args!($($arg)+));
        }
    }
}

/// Same as [log], but silently fails on panic instead of
/// panicking.
macro_rules! log_no_panic {
    ($($arg:tt)+) => {
        echo_no_panic!("{}: {}", module_path!(), format_args!($($arg)+));
    }
}

/// Like [log], but prints the message only if debugging is enabled for the
/// module where it is used. This can be used for verbose things only needed
/// when debugging.
macro_rules! log_dbg {
    ($($arg:tt)+) => {
        if $crate::log::ENABLED_MODULES.contains(&module_path!()) {
            log!($($arg)*);
        }
    }
}

/// Like [log], but messages only log once and cannot have formatting.
/// To be used for log messages that are known to spam the log file (like those
/// logged every frame).
macro_rules! log_once {
    ($msg:literal) => {{
        static LOG_ONCE: std::sync::Once = std::sync::Once::new();
        LOG_ONCE.call_once(|| {
            log!("{} [this log will only be shown once]", $msg);
        });
    }};
}

/// Like [log], but only writes to the log file, not to the terminal (or on
/// Android, logcat). To be used for per-call traces that can emit thousands of
/// lines per second.
#[allow(unused_macros)]
macro_rules! log_file_only {
    ($($arg:tt)+) => {
        {
            let line = format!("{}: {}\n", module_path!(), format_args!($($arg)+));
            let _ = std::io::Write::write_all(&mut $crate::log::get_log_file(), line.as_bytes());
        }
    };
}

/// Print a message (with implicit newline). This should be used for all
/// touchHLE output that isn't coming from the app itself.
///
/// Prefer use [log] or [log_dbg] for errors and warnings during emulation.
macro_rules! echo {
    ($($arg:tt)+) => {
        $crate::log::echo_line(&format!($($arg)+))
    };
    () => {
        $crate::log::echo_line("")
    }
}

/// Write a line to the terminal (or on Android, logcat) and the log file.
/// Use [echo] rather than calling this directly.
///
/// Apps sometimes trigger the same warning on every frame (e.g. by calling an
/// unimplemented function). Writing it out every time slows touchHLE down,
/// especially on Android, and makes the log file huge, so after a few
/// identical lines in a row, the rest are only counted.
pub fn echo_line(line: &str) {
    use std::hash::{Hash, Hasher};
    use std::sync::Mutex;

    /// How many times in a row the same line is written out before further
    /// repeats are only counted.
    const MAX_REPEATS: u32 = 5;
    /// Hash of the last line and how many times in a row it has been seen.
    static LAST_LINE: Mutex<(u64, u32)> = Mutex::new((0, 0));

    let mut hasher = std::hash::DefaultHasher::new();
    line.hash(&mut hasher);
    let hash = hasher.finish();

    let previous_hidden = {
        let Ok(mut last) = LAST_LINE.lock() else {
            write_line(line);
            return;
        };
        if last.0 == hash && last.1 > 0 {
            last.1 += 1;
            if last.1 > MAX_REPEATS + 1 {
                return;
            } else if last.1 == MAX_REPEATS + 1 {
                drop(last);
                write_line("(The previous line is being repeated; further repeats are hidden.)");
                return;
            }
            None
        } else {
            let hidden = last.1.saturating_sub(MAX_REPEATS);
            *last = (hash, 1);
            (hidden > 0).then_some(hidden)
        }
    };
    if let Some(hidden) = previous_hidden {
        write_line(&format!(
            "(The previous line was repeated {} more times.)",
            hidden
        ));
    }
    write_line(line);
}

fn write_line(line: &str) {
    #[cfg(target_os = "android")]
    {
        sdl2::log::log(line);
    }
    #[cfg(not(target_os = "android"))]
    eprintln!("{}", line);

    if !LOG_TO_FILE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    use std::io::Write;
    let mut log_file = get_log_file();
    let _ = log_file.write_all(line.as_bytes());
    let _ = log_file.write_all(b"\n");
}

/// Same as [echo], but silently fails on panic instead of
/// panicking.
macro_rules! echo_no_panic {
    ($($arg:tt)*) => {
        {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                echo!($($arg)*);
            }));
        }
    }
}

/// Put modules to enable [log_dbg] for here, e.g. "touchHLE::mem" to see when
/// memory is allocated and freed.
pub const ENABLED_MODULES: &[&str] = &[];
