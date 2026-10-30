#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Record native macOS CFUUID behavior; not an old iPhone/ARM32 test.

Uses public CoreFoundation APIs. Outputs UTF-16 parser fixtures and an
ownership observation record, without releasing any borrowed constants.
"""
import argparse
import ctypes as c
import json
import pathlib
import platform


class UUIDBytes(c.Structure):
    _fields_ = [("bytes", c.c_ubyte * 16)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--manifest", type=pathlib.Path, required=True)
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("This probe requires macOS")
    cf = c.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
    def bind(name, result, parameters):
        function = getattr(cf, name)
        function.restype, function.argtypes = result, parameters
        return function
    make_string = bind("CFStringCreateWithCharacters", c.c_void_p, [c.c_void_p, c.POINTER(c.c_uint16), c.c_long])
    parse = bind("CFUUIDCreateFromString", c.c_void_p, [c.c_void_p, c.c_void_p])
    raw = bind("CFUUIDGetUUIDBytes", UUIDBytes, [c.c_void_p])
    release = bind("CFRelease", None, [c.c_void_p])
    count = bind("CFGetRetainCount", c.c_long, [c.c_void_p])
    create_bytes = bind("CFUUIDCreateFromUUIDBytes", c.c_void_p, [c.c_void_p, UUIDBytes])
    constant = bind("CFUUIDGetConstantUUIDWithBytes", c.c_void_p, [c.c_void_p] + [c.c_ubyte] * 16)
    canonical = "550E8400-E29B-41D4-A716-446655440000"
    cases = ["", canonical, canonical.lower(), "{" + canonical + "}",
             canonical.replace("-", ""), canonical.replace("-", "+"),
             "zz" + canonical, "garbage" + canonical, "💥" + canonical,
             "5", "55", "550e8400", "zzzz", "not a UUID", "gg", "0",
             canonical + " trailing", "z" * 98 + "ABCD", "z" * 99 + "ABCD",
             "z" * 100 + canonical, "x" * 120 + canonical,
             "Ā" + canonical]
    units = [list(int.from_bytes(s.encode("utf-16-le")[i:i+2], "little")
                  for i in range(0, len(s.encode("utf-16-le")), 2)) for s in cases]
    units.extend([[0xd800] + units[1], [0xdc00], [0, 0x35, 0x35]])
    lines = ["# Observed native macOS CFUUID parser; UTF-16 units / UUID bytes",
             "# '-' input means empty; '-' result means NULL"]
    for chars in units:
        data = (c.c_uint16 * len(chars))(*chars)
        text = make_string(None, data, len(chars))
        value = parse(None, text)
        result = bytes(raw(value).bytes).hex() if value else "-"
        if value:
            release(value)
        release(text)
        lines.append(f"{''.join(f'{x:04x}' for x in chars) or '-'}\t{result}")
    nil_result = parse(None, None)
    if nil_result:
        release(nil_result)
    root_bytes = bytes.fromhex("01a273b4c5d647e8a9fa0b1c2d3e4fc0")
    root = constant(None, *root_bytes)
    before = count(root)
    owned = create_bytes(None, raw(root))
    during = count(root)
    same = root == owned
    release(owned)
    after = count(root)
    # Observe promotion before release; historical implementations may not
    # retain an ordinary object, so do not dereference the borrowed pointer
    # after releasing that object's owner in this host probe.
    data = UUIDBytes((c.c_ubyte * 16)(*bytes.fromhex("23a273b4c5d647e8a9fa0b1c2d3e4fc1")))
    ordinary = create_bytes(None, data)
    ordinary_before = count(ordinary)
    borrowed = constant(None, *bytes(data.bytes))
    ordinary_after = count(ordinary)
    promotion_same = ordinary == borrowed
    release(ordinary)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(lines) + "\n")
    args.manifest.write_text(json.dumps({
        "host": platform.platform(), "scope": "Native macOS observation, not historical iPhone validation",
        "parser_cases": len(units), "null_input_returns_null": not nil_result,
        "constant_reused_by_create": same, "constant_count_before_during_after": [before, during, after],
        "ordinary_get_same_object": promotion_same,
        "ordinary_count_before_after_get": [ordinary_before, ordinary_after],
        "promotion_after_release_not_dereferenced": True,
    }, indent=2) + "\n")
    print(f"Recorded {len(units)} native CFUUID parser cases")


if __name__ == "__main__":
    main()
