#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""macOS same-process SDL virtual-controller smoke driver (no physical pad).

Default: inspect the release executable without launching it.
--native-fixture builds a temporary ARM guest and uses readiness handshakes;
it adds --system-version=7.0 and checks UIDevice.systemVersion.floatValue
before requesting GameController, matching P2G discovery. It returns 0 only
after guest assertions and a clean guest exit. The injector
requests foreground activation once before sending input; leave other windows
untouched until completion (normal focus neutralization remains enabled).
  python3 dev-scripts/controller-smoke-test.py
  python3 dev-scripts/controller-smoke-test.py --native-fixture --delay 0
  python3 dev-scripts/controller-smoke-test.py --app Podroid_apps/p2g.ipa
  python3 dev-scripts/controller-smoke-test.py --app 'Podroid_apps/GTA罪恶都市.ipa'

Use --delay to allow loading, and append emulator options after --. Keep the
window visible: --headless bypasses the real window/controller path. A fixed
sequence attaches a game-controller joystick, presses A, moves left X, presses
D-pad right, inverted right Y, independent mixed triggers, RightShoulder,
and two Start presses (including held/repeated down), releases each, and detaches. Save logs outside the temporary build
with --log. Injection alone is NOT a guest test: without --expect (repeatable
regexes for guest-produced evidence), observation runs return 2, never PASS.
Do not use injector messages or host connection messages as guest expectations.
For P2G verify GC enumeration, values AND block callbacks; for GTA verify mapped
touch began/moved/ended at configured coordinates. A visual run requires human
assessment; timings do not establish that either game reached gameplay.

Static SDL's public virtual functions are dead-stripped in current builds, but
its private *_REAL implementations survive in the dynamic API table. Resolve
these from this exact executable's nm table plus dyld's runtime ASLR slide.
No executable patching, second SDL instance, or Rust changes are involved.
This is intentionally a developer-only fallback, not a stable SDL ABI. Stripped
binaries fail preflight. Durable alternative: build matching vendor/SDL shared
and route ALL host SDL calls via its supported SDL_DYNAMIC_API override; merely
loading another SDL alongside static SDL creates separate joystick state.

Only macOS is supported. This cannot validate real Android controller drivers,
OpenGL, or physical hardware. No app assets are copied into the harness.
"""

import argparse
import hashlib
import importlib.util
import plistlib
import shutil
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
import tempfile
import time
import json

ROOT = Path(__file__).resolve().parents[1]
FUNCTIONS = {
    "WasInit": "unsigned int (*)(unsigned int)",
    "LockJoysticks": "void (*)(void)",
    "UnlockJoysticks": "void (*)(void)",
    "JoystickAttachVirtual": "int (*)(int,int,int,int)",
    "JoystickDetachVirtual": "int (*)(int)",
    "JoystickOpen": "void *(*)(int)",
    "JoystickClose": "void (*)(void *)",
    "JoystickSetVirtualAxis": "int (*)(void *,int,short)",
    "JoystickSetVirtualButton": "int (*)(void *,int,unsigned char)",
    "IsGameController": "int (*)(int)",
}

SOURCE = r'''
#import <AppKit/AppKit.h>
#include <mach-o/dyld.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <unistd.h>
@FUNCTIONS@
#define LOG(s) do { fprintf(stderr, "[controller-smoke] %s\n", s); fflush(stderr); } while (0)
static int gate(const char *path) {
    if (!*path) return 1;
    for (int i = 0; i < @GATE_POLLS@; ++i) {
        if (access(path, F_OK) == 0) return 1;
        usleep(50000);
    }
    LOG("ERROR guest readiness timeout");
    return 0;
}
static void *drive(void *unused) {
    (void)unused;
    // SDL joystick operations are serialized with the emulator's event pump.
    for (int i = 0; i < 600; ++i) {
        LockJoysticks();
        unsigned int ready = WasInit(0x2000); // SDL_INIT_GAMECONTROLLER
        UnlockJoysticks();
        if (ready) goto ready;
        usleep(50000);
    }
    LOG("ERROR controller subsystem not initialized within 30 seconds");
    return NULL;
ready:
    if (!gate(@READY@)) return NULL;
    usleep(@DELAY@);
    LockJoysticks();
    int index = JoystickAttachVirtual(1, 6, 15, 0); // GAMECONTROLLER, SDL standard indices
    void *pad = index < 0 ? NULL : JoystickOpen(index);
    int recognized = index >= 0 && IsGameController(index);
    if (!pad || !recognized) {
        LOG("ERROR virtual device could not open as a game controller");
        if (pad) JoystickClose(pad);
        if (index >= 0) JoystickDetachVirtual(index);
        UnlockJoysticks();
        return NULL;
    }
    // SDL trigger neutral is -32768; all other axes/buttons start at zero.
    int failed = JoystickSetVirtualAxis(pad, 4, -32768);
    failed |= JoystickSetVirtualAxis(pad, 5, -32768);
    UnlockJoysticks();
    LOG("attached recognized virtual controller");
    if (!gate(@HANDLERS@)) return NULL;
    // The GUI-less guest has no UIApplication activation. Request real macOS
    // foreground focus once; do not inject focus events or bypass neutralization.
    @autoreleasepool {
        BOOL activated = [[NSRunningApplication currentApplication]
            activateWithOptions:NSApplicationActivateIgnoringOtherApps];
        if (!activated) LOG("ERROR macOS refused test app foreground activation");
        else LOG("requested test app foreground activation");
    }
    sleep(2);
    LOG("A down");
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 0, 1); UnlockJoysticks();
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 0, 0); UnlockJoysticks();
    LOG("A up");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualAxis(pad, 0, 24576); UnlockJoysticks();
    LOG("left X positive");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualAxis(pad, 0, 0); UnlockJoysticks();
    LOG("left X neutral");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 14, 1); UnlockJoysticks();
    LOG("D-pad right down");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 14, 0); UnlockJoysticks();
    LOG("D-pad right up");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualAxis(pad, 3, 32767); UnlockJoysticks();
    LOG("right Y positive (guest negative)");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualAxis(pad, 3, -32768); UnlockJoysticks();
    LOG("right Y negative (guest positive)");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualAxis(pad, 3, 0); UnlockJoysticks();
    LOG("right Y neutral");
    sleep(1);
    // Virtual joystick triggers use signed full range. SDL maps this to 0..32767.
    // Keep unequal simultaneous values and release independently to catch swaps,
    // coupling, shared-axis assumptions, and an extra signed normalization.
    LockJoysticks(); failed |= JoystickSetVirtualAxis(pad, 4, -16384); UnlockJoysticks();
    LOG("LT quarter RT zero");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualAxis(pad, 5, 16384); UnlockJoysticks();
    LOG("LT quarter RT three quarters");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualAxis(pad, 4, 32767); UnlockJoysticks();
    LOG("LT full RT three quarters");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualAxis(pad, 4, -32768); UnlockJoysticks();
    LOG("LT zero RT three quarters");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualAxis(pad, 5, -32768); UnlockJoysticks();
    LOG("LT zero RT zero");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 10, 1); UnlockJoysticks();
    LOG("RightShoulder down");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 6, 1); UnlockJoysticks();
    LOG("Start first down");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 6, 1); UnlockJoysticks();
    LOG("Start repeated down while held");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 6, 0); UnlockJoysticks();
    LOG("Start first up");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 10, 0); UnlockJoysticks();
    LOG("RightShoulder up (guest checks exactly one pause so far)");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 6, 1); UnlockJoysticks();
    LOG("Start second down");
    sleep(1);
    LockJoysticks(); failed |= JoystickSetVirtualButton(pad, 6, 0); UnlockJoysticks();
    LOG("Start second up");
    sleep(2); // Guest keeps pumping to detect repeated/incorrect release callbacks.
    LockJoysticks();
    JoystickClose(pad);
    failed |= JoystickDetachVirtual(index);
    if (failed) LOG("ERROR SDL rejected an input or detach");
    else LOG("sequence complete (injection only)");
    UnlockJoysticks();
    return NULL;
}
__attribute__((constructor)) static void begin(void) {
    LOG("injector loaded");
    intptr_t slide = 0;
    int found = 0;
    for (uint32_t i = 0; i < _dyld_image_count(); ++i) {
        if (_dyld_get_image_header(i)->filetype == MH_EXECUTE) {
            slide = _dyld_get_image_vmaddr_slide(i);
            found = 1;
            break;
        }
    }
    if (!found) { LOG("ERROR executable image missing"); return; }
    @ASSIGN@
    pthread_t thread;
    if (pthread_create(&thread, NULL, drive, NULL)) LOG("ERROR pthread_create failed");
    else pthread_detach(thread);
}
'''



GUEST = r'''
#include "system_headers.h"
#include <stdio.h>
@interface NSString (SmokeSystemVersion)
- (float)floatValue;
@end
@interface UIDevice : NSObject
+ (id)currentDevice;
- (NSString *)systemVersion;
@end
@interface NSDate : NSObject
+ (id)dateWithTimeIntervalSinceNow:(double)value;
@end
@interface NSRunLoop : NSObject
+ (id)currentRunLoop;
- (void)runUntilDate:(id)date;
@end
@interface NSObject (SmokeTypeChecks)
- (BOOL)isKindOfClass:(Class)class;
- (id)copy;
@end
@interface NSThread : NSObject
+ (BOOL)isMainThread;
@end
@interface GCController : NSObject
+ (NSArray *)controllers;
- (id)extendedGamepad;
- (BOOL)isAttached;
- (void)setControllerPausedHandler:(void (^)(id))handler;
@end
@interface GCControllerElement : NSObject
@end
@interface GCGamepad : NSObject
@end
@interface GCControllerAxisInput : GCControllerElement
- (float)value;
- (void)setValueChangedHandler:(void (^)(id,float))handler;
@end
@interface GCControllerButtonInput : GCControllerElement
- (float)value;
- (BOOL)isPressed;
- (void)setValueChangedHandler:(void (^)(id,float,BOOL))handler;
- (void)setPressedChangedHandler:(void (^)(id,float,BOOL))handler;
@end
@interface GCControllerDirectionPad : NSObject
- (GCControllerAxisInput *)xAxis;
- (GCControllerAxisInput *)yAxis;
- (void)setValueChangedHandler:(void (^)(id,float,float))handler;
@end
@interface GCExtendedGamepad : NSObject
- (GCControllerButtonInput *)buttonA;
- (GCControllerDirectionPad *)leftThumbstick;
- (GCControllerDirectionPad *)rightThumbstick;
- (GCControllerButtonInput *)leftTrigger;
- (GCControllerButtonInput *)rightTrigger;
- (GCControllerButtonInput *)rightShoulder;
- (GCControllerDirectionPad *)dpad;
- (void)setValueChangedHandler:(void (^)(id,id))handler;
@end
static unsigned seen, profiles, pressed;
static int bad;
static unsigned right_y, lt, rt, shoulder, shoulder_pressed, pauses, trigger_phase;
static unsigned connects, disconnects;
extern NSString *GCControllerDidConnectNotification;
extern NSString *GCControllerDidDisconnectNotification;
@interface SmokeControllerObserver : NSObject
- (void)connected:(NSNotification *)notification;
- (void)disconnected:(NSNotification *)notification;
@end
@implementation SmokeControllerObserver
- (void)connected:(NSNotification *)notification { (void)notification; ++connects; }
- (void)disconnected:(NSNotification *)notification { (void)notification; ++disconnects; }
@end
static int near(float a, float b) { return a > b - 0.002f && a < b + 0.002f; }
static void install(GCController *controller, GCExtendedGamepad *profile) {
    // Capturing an automatic local forces stack-block copying by the setter.
    // install returns before any input, so an uncopied block cannot be valid.
    int cookie = 0x1234;
    unsigned long controller_address = (unsigned long)controller;
    [[profile buttonA] setValueChangedHandler:^(id b, float value, BOOL down) {
        if (cookie != 0x1234 || value != [(GCControllerButtonInput *)b value] ||
            !!down != !![(GCControllerButtonInput *)b isPressed]) bad = 1;
        if (value != (down ? 1.0f : 0.0f)) bad = 1;
        seen |= down ? 1 : 2;
        printf("GUEST A value=%f pressed=%d cookie=%x\n", value, down, cookie);
    }];
    [[profile buttonA] setPressedChangedHandler:^(id b, float value, BOOL down) {
        if (cookie != 0x1234 || value != [(GCControllerButtonInput *)b value]) bad = 1;
        pressed |= down ? 1 : 2;
    }];
    [[[profile leftThumbstick] xAxis] setValueChangedHandler:^(id axis, float value) {
        if (cookie != 0x1234 || value != [(GCControllerAxisInput *)axis value]) bad = 1;
        if (value > 0.7f && value < 0.8f) seen |= 4;
        else if (value == 0 && (seen & 4)) seen |= 8;
        else bad = 1;
        printf("GUEST axis value=%f cookie=%x\n", value, cookie);
    }];
    [[profile dpad] setValueChangedHandler:^(id pad, float x, float y) {
        if (cookie != 0x1234 || x != [[(GCControllerDirectionPad *)pad xAxis] value] || y != 0) bad = 1;
        if (x == 1) seen |= 16;
        else if (x == 0 && (seen & 16)) seen |= 32;
        else bad = 1;
        printf("GUEST dpad x=%f y=%f cookie=%x\n", x, y, cookie);
    }];
    [[[profile rightThumbstick] yAxis] setValueChangedHandler:^(id axis, float value) {
        static const float expected[] = {-1, 1, 0};
        if (cookie != 0x1234 || ![NSThread isMainThread] || right_y >= 3 ||
            value != [(GCControllerAxisInput *)axis value] || value != expected[right_y]) bad = 1;
        ++right_y;
        printf("GUEST right Y inverted value=%f cookie=%x\n", value, cookie);
    }];
    [[profile leftTrigger] setValueChangedHandler:^(id b, float value, BOOL down) {
        static const float expected[] = {0.25f, 1, 0};
        if (cookie != 0x1234 || ![NSThread isMainThread] || lt >= 3 ||
            !near(value, expected[lt]) || value != [(GCControllerButtonInput *)b value] ||
            !!down != (value > 0.5f) || !!down != !![(GCControllerButtonInput *)b isPressed]) bad = 1;
        ++lt;
        printf("GUEST LT value=%f pressed=%d cookie=%x\n", value, down, cookie);
    }];
    [[profile rightTrigger] setValueChangedHandler:^(id b, float value, BOOL down) {
        static const float expected[] = {0.75f, 0};
        if (cookie != 0x1234 || ![NSThread isMainThread] || rt >= 2 ||
            !near(value, expected[rt]) || value != [(GCControllerButtonInput *)b value] ||
            !!down != (value > 0.5f) || !!down != !![(GCControllerButtonInput *)b isPressed]) bad = 1;
        ++rt;
        printf("GUEST RT value=%f pressed=%d cookie=%x\n", value, down, cookie);
    }];
    [[profile rightShoulder] setValueChangedHandler:^(id b, float value, BOOL down) {
        if (cookie != 0x1234 || ![NSThread isMainThread] || shoulder >= 2 ||
            value != (shoulder == 0 ? 1 : 0) || value != [(GCControllerButtonInput *)b value] ||
            !!down != !![(GCControllerButtonInput *)b isPressed]) bad = 1;
        if (!down && pauses != 1) bad = 1;
        ++shoulder;
        printf("GUEST RightShoulder value=%f pauses=%u cookie=%x\n", value, pauses, cookie);
    }];
    [[profile rightShoulder] setPressedChangedHandler:^(id b, float value, BOOL down) {
        if (cookie != 0x1234 || ![NSThread isMainThread] ||
            value != [(GCControllerButtonInput *)b value] || !!down != (value > 0.5f)) bad = 1;
        ++shoulder_pressed;
    }];
    // Captures only a primitive cookie: validates copied stack-block lifetime
    // without relying on retention of a captured Objective-C controller object.
    [controller setControllerPausedHandler:^(id pad) {
        if (cookie != 0x1234 || ![NSThread isMainThread] || ![pad isAttached] ||
            (unsigned long)pad != controller_address) bad = 1;
        ++pauses;
        if (pauses > 2) bad = 1;
        printf("GUEST pause edge=%u main=%d cookie=%x\n", pauses, [NSThread isMainThread], cookie);
    }];
    [profile setValueChangedHandler:^(id pad, id element) {
        if (cookie != 0x1234 || !pad || !element) bad = 1;
        ++profiles;
    }];
}
int main(void) {
    [NSAutoreleasePool new];
    // Compiler-generated copy properties send Objective-C messages to blocks.
    // Exercise that path as well as the C _Block_copy used by profile setters.
    int cookie = 73;
    void (^stack_block)(void) = ^{
        if (cookie != 73) bad = 1;
    };
    id copied = [(id)stack_block copy];
    [copied retain];
    [copied release];
    ((void (^)(void))copied)();
    [copied autorelease];
    id global_copy = [(id)^{ puts("GUEST global block callback PASS"); } copy];
    ((void (^)(void))global_copy)();
    [global_copy release];
    puts("GUEST Objective-C block copy/retain/release PASS");
    // P2G checks this public version gate before requesting native controllers.
    float version = [[[UIDevice currentDevice] systemVersion] floatValue];
    if (version != 7.0f) {
        printf("GUEST FAIL: UIDevice version gate expected=7.0 actual=%f\n", version); return 1;
    }
    puts("GUEST version gate PASS: UIDevice.systemVersion.floatValue=7.0 before GameController");
    // Do not query GCController until a connect notification arrives. This
    // catches bridges that wait for enumeration before sending notifications.
    id observer = [SmokeControllerObserver new];
    [[NSNotificationCenter defaultCenter] addObserver:observer selector:@selector(connected:)
        name:GCControllerDidConnectNotification object:nil];
    [[NSNotificationCenter defaultCenter] addObserver:observer selector:@selector(disconnected:)
        name:GCControllerDidDisconnectNotification object:nil];
    puts("GUEST ready: observer waiting for virtual attach");
    GCController *controller = nil;
    for (int i = 0; i < 4000; ++i) {
        [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
        NSArray *all = connects ? [GCController controllers] : [NSArray array];
        if (!controller && [all count]) {
            if ([all count] != 1) { puts("GUEST FAIL: unexpected controller count"); return 1; }
            controller = [[all objectAtIndex:0] retain];
            GCExtendedGamepad *profile = [controller extendedGamepad];
            if (!profile || ![controller isAttached] || [[profile buttonA] value] != 0 ||
                [[[profile leftThumbstick] xAxis] value] != 0) bad = 1;
            if ([[[profile rightThumbstick] yAxis] value] != 0 ||
                [[profile leftTrigger] value] != 0 || [[profile rightTrigger] value] != 0 ||
                [[profile rightShoulder] value] != 0) bad = 1;
            if (![[profile buttonA] isKindOfClass:[GCControllerElement class]] ||
                [[profile buttonA] isKindOfClass:[GCControllerAxisInput class]] ||
                ![profile isKindOfClass:[NSObject class]] || [profile isKindOfClass:[GCGamepad class]]) {
                puts("GUEST FAIL: public GameController type hierarchy"); return 1;
            }
            puts("GUEST type hierarchy: button is Element, not Axis; extended is NSObject, not Gamepad");
            install(controller, profile);
            puts("GUEST enumerated: handlers installed");
        }
        if (controller && [all count] != 0) {
            // Observe whole snapshots as well as individual callbacks. Each phase
            // holds the OTHER trigger unchanged, proving independent normalization.
            static const float expected[][2] = {{0.25f, 0}, {0.25f, 0.75f},
                                               {1, 0.75f}, {0, 0.75f}, {0, 0}};
            GCExtendedGamepad *profile = [controller extendedGamepad];
            float l = [[profile leftTrigger] value], r = [[profile rightTrigger] value];
            if (trigger_phase < 5 && near(l, expected[trigger_phase][0]) &&
                near(r, expected[trigger_phase][1])) {
                printf("GUEST trigger snapshot phase=%u LT=%f RT=%f\n", trigger_phase, l, r);
                ++trigger_phase;
            }
        }
        if (controller && [all count] == 0) {
            if (![controller isAttached] && seen == 63 && pressed == 3 && profiles >= 16 && right_y == 3 && lt == 3 && rt == 2 &&
                trigger_phase == 5 && shoulder == 2 && shoulder_pressed == 2 && pauses == 2 &&
                connects == 1 && disconnects == 1 && !bad) {
                printf("GUEST coverage rightY=%u LT=%u RT=%u mixed=%u shoulder=%u pressed=%u pauses=%u\n",
                       right_y, lt, rt, trigger_phase, shoulder, shoulder_pressed, pauses);
                puts("GUEST PASS: enumeration values copied-block callbacks removal"); return 0;
            }
            printf("GUEST FAIL: seen=%u pressed=%u profiles=%u bad=%d attached=%d\n",
                   seen, pressed, profiles, bad, [controller isAttached]);
            printf("GUEST FAIL: rightY=%u LT=%u RT=%u mixed=%u shoulder=%u pressed=%u pauses=%u\n",
                   right_y, lt, rt, trigger_phase, shoulder, shoulder_pressed, pauses); return 1;
        }
    }
    puts("GUEST FAIL: timeout"); return 1;
}
'''


def build_guest(directory, binary):
    # Reuse stub generation/startup only; never modify base TestApp.
    spec = importlib.util.spec_from_file_location("macos_fixture_builder", ROOT / "dev-scripts/macos-integration-tests.py")
    builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(builder)
    builder.ROOT = str(ROOT)
    builder.TOUCHHLE = str(binary)
    builder.OUT = str(directory)
    builder.STUBS = str(directory / "stubs")
    app = directory / "ControllerSmoke.app"
    app.mkdir()
    (app / "Info.plist").write_bytes(plistlib.dumps({
        "CFBundleIdentifier": "org.touchhle.controller-smoke",
        "CFBundleExecutable": "ControllerSmoke", "CFBundleName": "ControllerSmoke",
        "CFBundleVersion": "1", "CFBundlePackageType": "APPL",
    }))
    source = directory / "guest.m"
    source.write_text(GUEST)
    start = directory / "start.s"
    start.write_text(builder.START_S)
    command(["xcrun", "clang", "--target=armv7-apple-ios5.0", "-c", str(start), "-o", str(directory / "start.o")])
    links = builder.make_stubs()
    command(["xcrun", "clang", *builder.CFLAGS, *builder.OBJC_FLAGS, "-fblocks",
             "-I" + str(ROOT / "tests/TestApp_source"), "-c", str(source), "-o", str(directory / "guest.o")])
    command(["xcrun", "clang", *builder.CFLAGS, "-nostartfiles", "-e", "start",
             str(directory / "guest.o"), str(directory / "start.o"), *links, "-o", str(app / "ControllerSmoke")])
    return app


def command(args):
    return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/Podroid")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--app", type=Path)
    mode.add_argument("--native-fixture", action="store_true", help="build and assert an ARM guest using public GameController APIs")
    parser.add_argument("--delay", type=float, default=8)
    parser.add_argument("--timeout", type=float, default=55)
    parser.add_argument("--log", type=Path, default=ROOT / "target/controller-smoke-test.log")
    parser.add_argument("--expect", action="append", default=[])
    parser.add_argument("--ready-file", type=Path,
                        help="wait for this external marker before attaching a controller to a game")
    parser.add_argument("options", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("macOS is required")
    if not 0 <= args.delay <= 600 or args.timeout < args.delay + 32:
        parser.error("delay must be 0..600 seconds; timeout must allow delay + 32 seconds")
    if args.ready_file and args.native_fixture:
        parser.error("--ready-file is for game observation, not the automated native fixture")
    binary = args.binary.resolve(strict=True)
    description = command(["file", str(binary)]).strip()
    if platform.machine() not in description or "universal" in description:
        parser.error("use a thin executable matching this Python process architecture")
    symbols = dict((name, int(address, 16)) for address, name in re.findall(
        r"^([0-9a-fA-F]+) [Tt] _SDL_(\w+)_REAL$", command(["nm", str(binary)]), re.M))
    missing = FUNCTIONS.keys() - symbols.keys()
    if missing:
        parser.error("missing private SDL symbols: " + ", ".join(sorted(missing)))
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    print(description)
    print("Static SDL private entry points available; SHA256 " + digest)
    if not args.app and not args.native_fixture:
        print("Inspection only. Supply --app to inject; see --help for limitations.")
        return 0
    app = args.app.resolve(strict=True) if args.app else None
    options = args.options[1:] if args.options[:1] == ["--"] else args.options
    if any(option == "--headless" or option.startswith("--dump") for option in options):
        parser.error("controller tests require the real window/event loop")
    if args.native_fixture:
        options = [*options, "--system-version=7.0"]
    expectations = [re.compile(pattern, re.M) for pattern in args.expect]
    args.log.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="controller-smoke-") as directory:
        # Run an immutable snapshot: other agents may rebuild the release binary.
        host = Path(directory) / "ControllerSmokeHost.app" / "Contents"
        (host / "MacOS").mkdir(parents=True)
        (host / "Info.plist").write_bytes(plistlib.dumps({
            "CFBundleIdentifier": "org.touchhle.controller-smoke-host",
            "CFBundleExecutable": "Podroid", "CFBundleName": "ControllerSmokeHost",
            "CFBundlePackageType": "APPL", "CFBundleVersion": "1",
            "SDL_FILESYSTEM_BASE_DIR_TYPE": "parent",
        }))
        snapshot = host / "MacOS" / "Podroid"
        shutil.copy2(binary, snapshot)
        if hashlib.sha256(snapshot.read_bytes()).hexdigest() != digest:
            parser.error("binary changed while copying; retry after build")
        # Resolve again from the immutable copy, in case a build raced preflight.
        symbols = dict((name, int(address, 16)) for address, name in re.findall(
            r"^([0-9a-fA-F]+) [Tt] _SDL_(\w+)_REAL$", command(["nm", str(snapshot)]), re.M))
        if FUNCTIONS.keys() - symbols.keys():
            parser.error("snapshot is missing required SDL symbols")
        if args.native_fixture:
            app = build_guest(Path(directory), snapshot)
            expectations = [
                re.compile(r"^GUEST version gate PASS: UIDevice.systemVersion.floatValue=7.0 before GameController$", re.M),
                re.compile(r"^GUEST PASS: enumeration values copied-block callbacks removal$", re.M),
            ]
        ready = args.ready_file.resolve() if args.ready_file else Path(directory) / "ready"
        handlers = Path(directory) / "handlers"
        source = Path(directory) / "inject.m"
        dylib = Path(directory) / "inject.dylib"
        declarations = []
        assignments = []
        for name, signature in FUNCTIONS.items():
            declarations.append("static " + signature.replace("(*)", "(*" + name + ")") + ";")
            assignments.append(f"{name} = ({signature})(slide + 0x{symbols[name]:x});")
        source.write_text(SOURCE.replace("@FUNCTIONS@", "\n".join(declarations))
                          .replace("@ASSIGN@", "\n".join(assignments))
                          .replace("@DELAY@", str(int(args.delay * 1000000)))
                          .replace("@GATE_POLLS@", str(int(max(1, args.timeout - 45) * 20)) if args.ready_file else "600")
                          .replace("@READY@", json.dumps(str(ready) if args.native_fixture or args.ready_file else ""))
                          .replace("@HANDLERS@", json.dumps(str(handlers) if args.native_fixture else "")))
        command(["xcrun", "clang", "-dynamiclib", "-Wall", "-Wextra", "-Werror", "-Wno-deprecated-declarations", str(source), "-framework", "AppKit", "-o", str(dylib)])
        environment = os.environ.copy()
        # SDL_DYNAMIC_API would route host input to a different SDL instance.
        environment.pop("SDL_DYNAMIC_API", None)
        environment["DYLD_INSERT_LIBRARIES"] = str(dylib)
        with args.log.open("w") as log:
            log.write(f"binary={binary}\nhost={host.parent}\nsha256={digest}\napp={app}\noptions={options!r}\n")
            log.flush()
            process = subprocess.Popen([str(snapshot), str(app), *options], cwd=ROOT,
                                       env=environment, stdout=log, stderr=subprocess.STDOUT,
                                       start_new_session=True)
            timed_out = False
            try:
                deadline = time.monotonic() + args.timeout
                while process.poll() is None:
                    if args.native_fixture:
                        progress = args.log.read_text(errors="replace")
                        if "GUEST ready: observer waiting for virtual attach" in progress:
                            ready.touch(exist_ok=True)
                        if "GUEST enumerated: handlers installed" in progress:
                            handlers.touch(exist_ok=True)
                    if time.monotonic() >= deadline:
                        raise subprocess.TimeoutExpired(process.args, args.timeout)
                    time.sleep(0.05)
            except subprocess.TimeoutExpired:
                timed_out = True
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
            except KeyboardInterrupt:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
                return 130
        output = args.log.read_text(errors="replace")
        print("Log: " + str(args.log.resolve()))
        complete = "[controller-smoke] sequence complete (injection only)" in output
        if not complete or "[controller-smoke] ERROR" in output or (not timed_out and process.returncode):
            print(f"FAIL: incomplete injection or emulator failure (exit={process.returncode}, timeout={timed_out}); inspect log")
            return 1
        # Exclude injector lines from guest evidence matching.
        guest = "\n".join(line for line in output.splitlines() if not line.startswith("[controller-smoke]"))
        if args.native_fixture and (timed_out or "GUEST FAIL:" in output):
            print("FAIL: native guest failed or did not exit normally")
            return 1
        if not expectations:
            print("INCONCLUSIVE: injection completed; guest behavior has not been asserted")
            return 2
        if not all(pattern.search(guest) for pattern in expectations):
            print("FAIL: required guest evidence missing")
            return 1
        print("PASS: native guest values, copied-block callbacks, enumeration and removal" if args.native_fixture
              else "PASS: injection completed and supplied evidence matched (review expectation quality)")
        return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as error:
        sys.exit(error.output or str(error))
    except OSError as error:
        sys.exit(str(error))
