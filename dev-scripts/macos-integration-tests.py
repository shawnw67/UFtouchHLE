#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Build TestApp with Apple's own toolchain and run its automated tests.

The normal integration test (tests/integration.rs, run by `cargo test`)
needs LLVM 12 and the common-3.0-sdk to be downloaded (see
tests/README.md). On a Mac with the Xcode command line tools, this script
gets close enough without downloading anything:

- Apple's clang and linker can still target armv7 iOS (5.0 or later), but
  not armv6 or iPhone OS 2/3, so only an armv7 binary is built, in ARM mode
  (the current linker drops the Thumb bit from data pointers to Thumb
  functions, which breaks TestApp's table of test functions).
- The current linker refuses to build stub dylibs without linking libSystem,
  so text-based stubs (.tbd files) are generated from touchHLE's symbol dump
  instead.
- The macOS SDK's headers stand in for the iPhone OS SDK's.
- crt1.o isn't available, so a tiny `start` routine is used.

Usage: dev-scripts/macos-integration-tests.py [--gui]
Run it from the repository root after `cargo build --release`. With --gui,
the UIKit test app is opened instead of running the automated tests.
The build happens in target/macos-integration-tests/.
"""

import glob
import os
import plistlib
import re
import shutil
import subprocess
import sys

ROOT = os.getcwd()
TOUCHHLE = os.path.join(ROOT, "target", "release", "touchHLE")
OUT = os.path.join(ROOT, "target", "macos-integration-tests")
STUBS = os.path.join(OUT, "stubs")
SOURCE = os.path.join(ROOT, "tests", "TestApp_source")
APP = os.path.join(OUT, "TestApp.app")

CFLAGS = [
    "--target=armv7-apple-ios5.0",
    "-marm",
    "-fno-stack-protector",
    "-DPRODUCT_iPhone",
    "-U_FORTIFY_SOURCE",
    "-D_FORTIFY_SOURCE=0",
    "-nodefaultlibs",
    "-w",
    "-Wno-incompatible-sysroot",
    "-Wno-error=implicit-function-declaration",
    "-Wno-error=int-conversion",
    "-Wno-error=incompatible-function-pointer-types",
]
OBJC_FLAGS = ["-ObjC", "-fno-objc-exceptions", "-fno-objc-arc", "-fno-objc-arc-exceptions"]

START_S = """
    .text
    .align 2
    .globl start
    .arm
start:
    ldr r0, [sp]            @ argc
    add r1, sp, #4          @ argv
    add r2, r1, r0, lsl #2
    add r2, r2, #4          @ envp
    bic sp, sp, #7
    bl _main
    bl _exit
"""


def run(cmd, **kwargs):
    result = subprocess.run(cmd, capture_output=True, text=True, **kwargs)
    if result.returncode != 0:
        sys.stdout.write(result.stdout)
        sys.stderr.write(result.stderr)
        sys.exit("Command failed: " + " ".join(cmd))
    return result


def tbd(install_name, symbols=(), classes=(), reexports=()):
    lines = [
        "--- !tapi-tbd",
        "tbd-version: 4",
        "targets: [ armv7-ios ]",
        "install-name: '%s'" % install_name,
        "current-version: 1",
        "compatibility-version: 1",
    ]
    if reexports:
        lines += [
            "reexported-libraries:",
            "  - targets: [ armv7-ios ]",
            "    libraries: [ %s ]" % ", ".join("'%s'" % r for r in reexports),
        ]
    lines += ["exports:", "  - targets: [ armv7-ios ]"]
    symbols = sorted(set(symbols)) or ["_touchHLE_stub_" + re.sub(r"\W", "_", install_name)]
    lines.append("    symbols: [ %s ]" % ", ".join("'%s'" % s for s in symbols))
    if classes:
        lines.append("    objc-classes: [ %s ]" % ", ".join(sorted(set(classes))))
    lines.append("...")
    return "\n".join(lines) + "\n"


def make_stubs():
    """Generate .tbd stubs for touchHLE's libraries and frameworks.
    Returns the linker arguments needed to link against them."""
    shutil.rmtree(STUBS, ignore_errors=True)
    os.makedirs(STUBS)
    symbols_path = os.path.join(STUBS, "SYMBOLS.txt")
    run([TOUCHHLE, "--dump=symbols", "--dump-file=" + symbols_path, "--headless"])

    # Same format as parsed by tests/integration.rs.
    libs = []
    current = None
    in_body = False
    for line in open(symbols_path):
        line = line.rstrip("\n")
        if line.startswith("// "):
            path = line[3:]
            if in_body or current is None:
                current = {"path": path, "aliases": [], "symbols": [], "classes": []}
                libs.append(current)
                in_body = False
            else:
                current["aliases"].append(path)
            continue
        in_body = True
        m = re.match(r"^\w[\w\s*]*?\b([A-Za-z_$][\w$]*)\(\) \{\}$", line) or re.match(
            r"^\w+ ([A-Za-z_$][\w$]*);$", line
        )
        if m:
            current["symbols"].append("_" + m.group(1))
            continue
        m = re.match(r"^@implementation (\w+)", line)
        if m:
            current["classes"].append(m.group(1))

    link_args = ["-L" + os.path.join(STUBS, "lib"), "-F" + os.path.join(STUBS, "Frameworks")]
    for lib in libs:
        path = lib["path"]
        if path.startswith("/.touchHLE"):
            continue  # the app picker's fake library
        name = path.rsplit("/", 1)[1]
        if path.startswith("/System/Library/Frameworks/"):
            directory = os.path.join(STUBS, "Frameworks", name + ".framework")
            os.makedirs(directory, exist_ok=True)
            with open(os.path.join(directory, name + ".tbd"), "w") as f:
                f.write(tbd(path, lib["symbols"], lib["classes"]))
            link_args += ["-framework", name]
        else:
            directory = os.path.join(STUBS, "lib")
            os.makedirs(directory, exist_ok=True)
            with open(os.path.join(directory, name[: -len(".dylib")] + ".tbd"), "w") as f:
                f.write(tbd(path, lib["symbols"], lib["classes"]))
            for alias in lib["aliases"]:
                alias_name = alias.rsplit("/", 1)[1]
                with open(os.path.join(directory, alias_name[: -len(".dylib")] + ".tbd"), "w") as f:
                    f.write(tbd(alias, reexports=[path]))
            link_args.append("-l" + name[len("lib") : -len(".dylib")])

    # The bundled libstdc++ has no platform information, which the current
    # linker rejects, so link against a stub of it too.
    libstdcxx = os.path.join(ROOT, "touchHLE_dylibs", "libstdc++.6.0.9.dylib")
    exported = run(["nm", "-arch", "armv7", "-gU", libstdcxx]).stdout.split("\n")
    with open(os.path.join(STUBS, "lib", "libstdc++.6.0.9.tbd"), "w") as f:
        f.write(tbd("/usr/lib/libstdc++.6.dylib", [l.split()[-1] for l in exported if l.strip()]))
    link_args.append("-lstdc++.6.0.9")
    return link_args


def build_app(link_args):
    shutil.rmtree(APP, ignore_errors=True)
    shutil.copytree(os.path.join(ROOT, "tests", "TestApp.app"), APP)
    objects = []
    start = os.path.join(OUT, "start.s")
    with open(start, "w") as f:
        f.write(START_S)
    objects.append(os.path.join(OUT, "start.o"))
    run(["clang", "--target=armv7-apple-ios5.0", "-Wno-incompatible-sysroot", "-c", start, "-o", objects[-1]])
    for source in sorted(glob.glob(os.path.join(SOURCE, "*.cpp"))):
        objects.append(os.path.join(OUT, os.path.basename(source) + ".o"))
        run(["clang"] + CFLAGS + ["-c", source, "-o", objects[-1]])
    for source in sorted(glob.glob(os.path.join(SOURCE, "*.m")) + glob.glob(os.path.join(SOURCE, "*.c"))):
        objects.append(os.path.join(OUT, os.path.basename(source) + ".o"))
        run(["clang"] + CFLAGS + OBJC_FLAGS + ["-c", source, "-o", objects[-1]])
    run(["clang"] + CFLAGS + ["-nostartfiles", "-e", "start"] + objects + link_args + ["-o", os.path.join(APP, "TestApp")])


def main():
    if not os.path.exists(TOUCHHLE):
        sys.exit("Build touchHLE first: cargo build --release")
    os.makedirs(OUT, exist_ok=True)
    build_app(make_stubs())
    if "--gui" in sys.argv[1:]:
        os.execv(TOUCHHLE, [TOUCHHLE, APP])
    result = subprocess.run([TOUCHHLE, APP, "--headless", "--args", "--cli-tests"], capture_output=True, text=True)
    output = result.stdout + result.stderr
    summary = [l for l in output.split("\n") if l.startswith("Passed ")]
    failures = [l for l in output.split("\n") if "FAIL" in l]
    for line in failures:
        print(line)
    print(summary[-1] if summary else output[-3000:])
    if result.returncode != 0:
        sys.exit(result.returncode)
    # Fortified overflows must fail through the actual ARM guest export.
    # Require the precise diagnostic, not merely an arbitrary crash.
    for mode in ("--fortify-overflow", "--fortify-unterminated", "--fortify-zero"):
        rejected = subprocess.run(
            [TOUCHHLE, APP, "--headless", "--args", mode],
            capture_output=True, text=True, timeout=30,
        )
        failure = rejected.stdout + rejected.stderr
        if (rejected.returncode == 0 or "__strncat_chk: buffer overflow" not in failure
                or "FORTIFY_RETURNED_UNEXPECTEDLY" in failure):
            sys.exit("Fortified guest failure check failed: " + mode + "\n" + failure[-3000:])
    print("PASS 3 fortified ARM guest rejection cases")
    exhausted = subprocess.run(
        [TOUCHHLE, APP, "--headless", "--args", "--allocation-exhaustion"],
        capture_output=True, text=True, timeout=30,
    )
    exhaustion_output = exhausted.stdout + exhausted.stderr
    if (exhausted.returncode != 0 or
            "PASS allocation exhaustion and reallocf release" not in exhaustion_output):
        sys.exit("Guest allocation exhaustion check failed:\n" + exhaustion_output[-3000:])
    print("PASS ARM guest address-space exhaustion and reallocf release")


if __name__ == "__main__":
    main()
