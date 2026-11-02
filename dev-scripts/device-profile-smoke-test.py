#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Check device-profile geometry from a minimal ARM guest on macOS.

Run after cargo build --release:
  python3 dev-scripts/device-profile-smoke-test.py

Reuses the macOS integration builder's startup and symbol stubs, without
building or changing TestApp. Each case starts a real emulator window and
asserts UIScreen bounds/currentMode.size and UIWindow frame/bounds in guest
code, then exits. No input, focus handling, screenshots, controller injection,
app assets, or physical device are needed. Temporary fixtures are deleted;
per-case logs remain in target/device-profile-smoke-tests/.
"""

import argparse
import hashlib
import importlib.util
import os
from pathlib import Path
import plistlib
import signal
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
# name, profile, supported families, tall-screen declaration, points, pixels
CASES = [
    ("legacy-phone-compat", "iPhone6,1", [1], False, (320, 480), (640, 960)),
    ("tall-phone-opt-in", "iPhone6,1", [1], True, (320, 568), (640, 1136)),
    ("compact-manual", "iPhone4,1", [1], True, (320, 480), (640, 960)),
    ("iphone-six", "iPhone7,2", [1], (375, 667), (375, 667), (750, 1334)),
    ("iphone-six-plus", "iPhone7,1", [1], (414, 736), (414, 736), (1242, 2208)),
    ("plus-tall-compat", "iPhone7,1", [1], True, (320, 568), (640, 1136)),
    ("plus-legacy-compat", "iPhone7,1", [1], False, (320, 480), (640, 960)),
    ("retina-ipad-universal", "iPad3,1", [1, 2], False, (768, 1024), (1536, 2048)),
    ("ipad-unsupported-phone-fallback", "iPad3,1", [1], False, (320, 480), (320, 480)),
]

GUEST = r'''
#include "system_headers.h"
#include <stdio.h>
#include <stdlib.h>
@interface UIScreenMode : NSObject
- (CGSize)size;
@end
@interface UIScreen (SmokeGeometry)
- (CGFloat)scale;
- (CGRect)bounds;
- (UIScreenMode *)currentMode;
@end
static int rectangle(CGRect rect, float width, float height) {
    return rect.origin.x == 0 && rect.origin.y == 0 &&
           rect.size.width == width && rect.size.height == height;
}
int main(int argc, char **argv) {
    [NSAutoreleasePool new];
    if (argc != 5) { puts("GUEST FAIL: expected four dimensions"); return 1; }
    int width = atoi(argv[1]), height = atoi(argv[2]);
    int pixelWidth = atoi(argv[3]), pixelHeight = atoi(argv[4]);
    UIScreen *screen = [UIScreen mainScreen];
    UIScreenMode *mode = [screen currentMode];
    CGRect bounds = [screen bounds];
    CGSize pixels = [mode size];
    float scale = [screen scale];
    UIWindow *window = [[UIWindow alloc] initWithFrame:bounds];
    CGRect frame = [window frame], local = [window bounds];
    printf("GUEST geometry screen=(%f,%f %fx%f) pixels=%fx%f window=(%f,%f %fx%f) local=(%f,%f %fx%f)\n",
           bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height,
           pixels.width, pixels.height, frame.origin.x, frame.origin.y,
           frame.size.width, frame.size.height, local.origin.x, local.origin.y,
           local.size.width, local.size.height);
    if (!screen || !mode || !window || !rectangle(bounds, width, height) ||
        pixels.width != pixelWidth || pixels.height != pixelHeight ||
        scale != (float)pixelWidth / width ||
        !rectangle(frame, width, height) || !rectangle(local, width, height)) {
        printf("GUEST FAIL: expected points=%dx%d pixels=%dx%d\n",
               width, height, pixelWidth, pixelHeight);
        return 1;
    }
    puts("GUEST PASS: screen bounds, mode pixels and window geometry");
    return 0;
}
'''


def command(args):
    return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT)


def build_guest(directory, binary):
    spec = importlib.util.spec_from_file_location(
        "device_profile_builder", ROOT / "dev-scripts/macos-integration-tests.py")
    builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(builder)
    builder.ROOT = str(ROOT)
    builder.TOUCHHLE = str(binary)
    builder.OUT = str(directory)
    builder.STUBS = str(directory / "stubs")
    links = builder.make_stubs()
    source = directory / "guest.m"
    source.write_text(GUEST)
    start = directory / "start.s"
    start.write_text(builder.START_S)
    command(["xcrun", "clang", "--target=armv7-apple-ios5.0", "-c", str(start),
             "-o", str(directory / "start.o")])
    command(["xcrun", "clang", *builder.CFLAGS, *builder.OBJC_FLAGS,
             "-I" + str(ROOT / "tests/TestApp_source"), "-c", str(source),
             "-o", str(directory / "guest.o")])
    guest = directory / "DeviceProfileSmoke"
    command(["xcrun", "clang", *builder.CFLAGS, "-nostartfiles", "-e", "start",
             str(directory / "guest.o"), str(directory / "start.o"), *links,
             "-o", str(guest)])
    return guest


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/Podroid")
    parser.add_argument("--output", type=Path, default=ROOT / "target/device-profile-smoke-tests")
    parser.add_argument("--timeout", type=float, default=20)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("macOS is required")
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="device-profile-smoke-") as temporary:
        directory = Path(temporary)
        # Keep all cases on the same executable despite concurrent builds.
        snapshot = directory / "Podroid"
        shutil.copy2(binary, snapshot)
        digest = hashlib.sha256(snapshot.read_bytes()).hexdigest()
        print("Binary SHA256: " + digest, flush=True)
        guest = build_guest(directory, snapshot)
        failures = 0
        environment = os.environ.copy()
        environment.pop("DYLD_INSERT_LIBRARIES", None)
        environment.pop("SDL_DYNAMIC_API", None)
        for name, profile, families, tall, points, pixels in CASES:
            app = directory / (name + ".app")
            app.mkdir()
            shutil.copy2(guest, app / "DeviceProfileSmoke")
            metadata = {
                "CFBundleIdentifier": "org.touchhle.device-profile-smoke." + name,
                "CFBundleExecutable": "DeviceProfileSmoke",
                "CFBundleName": "DeviceProfileSmoke", "CFBundleVersion": "1",
                "CFBundlePackageType": "APPL", "UIDeviceFamily": families,
            }
            if tall:
                declared = tall if isinstance(tall, tuple) else (320, 568)
                metadata["UILaunchImages"] = [{"UILaunchImageSize": "{%d,%d}" % declared}]
            (app / "Info.plist").write_bytes(plistlib.dumps(metadata))
            log = output / (name + ".log")
            with log.open("w") as stream:
                stream.write(f"binary={binary}\nsha256={digest}\nprofile={profile}\n"
                             f"families={families} tall={tall} points={points} pixels={pixels}\n")
                stream.flush()
                process = subprocess.Popen(
                    [str(snapshot), str(app), "--device-profile=" + profile, "--args",
                     *(str(dimension) for dimension in (*points, *pixels))],
                    cwd=ROOT, env=environment, stdout=stream, stderr=subprocess.STDOUT,
                    start_new_session=True)
                timed_out = False
                try:
                    process.wait(timeout=args.timeout)
                except subprocess.TimeoutExpired:
                    timed_out = True
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                except KeyboardInterrupt:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                    raise
            text = log.read_text(errors="replace")
            passed = (not timed_out and process.returncode == 0
                      and "GUEST PASS: screen bounds, mode pixels and window geometry" in text
                      and "GUEST FAIL:" not in text)
            print(f"{'PASS' if passed else 'FAIL'} {name}: {points[0]}x{points[1]} points, "
                  f"{pixels[0]}x{pixels[1]} pixels", flush=True)
            if not passed:
                failures += 1
                print(f"  exit={process.returncode} timeout={timed_out}; log: {log}", flush=True)
        print(f"Passed {len(CASES) - failures}/{len(CASES)} device-profile guest cases. Logs: {output}")
        return 1 if failures else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as error:
        sys.exit(error.output or str(error))
    except OSError as error:
        sys.exit(str(error))
    except KeyboardInterrupt:
        sys.exit(130)
