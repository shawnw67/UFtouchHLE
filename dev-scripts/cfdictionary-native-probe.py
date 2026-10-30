#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Observe native CFDictionary equal-key replacement on macOS."""

import ctypes as c
import json
import platform
import sys

if sys.platform != "darwin":
    sys.exit("This probe requires macOS CoreFoundation")

cf = c.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
Retain = c.CFUNCTYPE(c.c_void_p, c.c_void_p, c.c_void_p)
Release = c.CFUNCTYPE(None, c.c_void_p, c.c_void_p)
Equal = c.CFUNCTYPE(c.c_ubyte, c.c_void_p, c.c_void_p)
Hash = c.CFUNCTYPE(c.c_ulong, c.c_void_p)
counts = {"retain": 0, "release": 0}


@Retain
def retain(_allocator, value):
    counts["retain"] += 1
    return value


@Release
def release(_allocator, _value):
    counts["release"] += 1


@Equal
def equal(left, right):
    return c.cast(left, c.POINTER(c.c_int))[0] == c.cast(right, c.POINTER(c.c_int))[0]


@Hash
def hash_key(_value):
    return 1


class KeyCallbacks(c.Structure):
    _fields_ = [
        ("version", c.c_long),
        ("retain", Retain),
        ("release", Release),
        ("description", c.c_void_p),
        ("equal", Equal),
        ("hash", Hash),
    ]


cf.CFDictionaryCreateMutable.argtypes = [c.c_void_p, c.c_long, c.POINTER(KeyCallbacks), c.c_void_p]
cf.CFDictionaryCreateMutable.restype = c.c_void_p
cf.CFDictionarySetValue.argtypes = [c.c_void_p, c.c_void_p, c.c_void_p]
cf.CFDictionaryGetKeysAndValues.argtypes = [c.c_void_p, c.POINTER(c.c_void_p), c.c_void_p]
cf.CFRelease.argtypes = [c.c_void_p]

callbacks = KeyCallbacks(0, retain, release, None, equal, hash_key)
first, second = c.c_int(5), c.c_int(5)
value1, value2 = c.c_int(1), c.c_int(2)
dictionary = cf.CFDictionaryCreateMutable(None, 0, c.byref(callbacks), None)
if not dictionary:
    sys.exit("CFDictionaryCreateMutable returned NULL")
cf.CFDictionarySetValue(dictionary, c.byref(first), c.byref(value1))
after_insert = counts.copy()
cf.CFDictionarySetValue(dictionary, c.byref(second), c.byref(value2))
keys = (c.c_void_p * 1)()
cf.CFDictionaryGetKeysAndValues(dictionary, keys, None)
after_replace = counts.copy()
original_preserved = keys[0] == c.addressof(first)
replacement_stored = keys[0] == c.addressof(second)
cf.CFRelease(dictionary)
print(json.dumps({
    "platform": platform.platform(),
    "machine": platform.machine(),
    "after_insert": after_insert,
    "after_replace": after_replace,
    "after_release": counts,
    "original_key_preserved": original_preserved,
    "replacement_key_stored": replacement_stored,
}, indent=2))
