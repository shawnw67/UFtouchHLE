#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Compare a supplied historical Apple strerror source with the current Mac.

Download SOURCE_URL separately, preserving the source's BSD notice. This is a
local validation probe, not a historical iPhone libc or physical-device test.
The historical function uses the host sys_errlist for positive error texts.
"""
import argparse
import ctypes
import hashlib
import json
import pathlib
import platform
import subprocess
import tempfile

SOURCE_URL = "https://raw.githubusercontent.com/apple-oss-distributions/Libc/0d7a000256faa8e750c97ebd703e5d01a5c26fb6/string/FreeBSD/strerror.c"
SOURCE_SHA256 = "90b2598f38daa3438218857138f8bc4c57fc35af57b4a685ae804b3ca14edb64"


def probe(function):
    function.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_size_t]
    function.restype = ctypes.c_int
    cases = []
    for code in (0, 22, -1, 9999):
        for capacity in (0, 1, 4, 128):
            buf = ctypes.create_string_buffer(b"Z" * 128, 129)
            ctypes.set_errno(33)
            result = function(code, buf, capacity)
            cases.append({"error": code, "capacity": capacity, "result": result,
                          "errno": ctypes.get_errno(),
                          "buffer_with_canaries_hex": buf.raw[:capacity + 2].hex()})
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=pathlib.Path, required=True)
    args = parser.parse_args()
    reference = args.reference.resolve()
    if platform.system() != "Darwin":
        parser.error("This probe requires macOS")
    if hashlib.sha256(reference.read_bytes()).hexdigest() != SOURCE_SHA256:
        parser.error("Reference source differs from the pinned Apple version")
    with tempfile.TemporaryDirectory(prefix="uf-darwin-error-") as directory:
        directory = pathlib.Path(directory)
        wrapper = directory / "wrapper.c"
        wrapper.write_text(
            "#define __FBSDID(text)\n#define strerror uf_historical_strerror\n"
            "#define strerror_r uf_historical_strerror_r\n#include "
            + json.dumps(str(reference)) + "\n"
        )
        library = directory / "oracle.dylib"
        subprocess.run(["clang", "-shared", "-fPIC", str(wrapper), "-o", str(library)], check=True)
        old = ctypes.CDLL(str(library), use_errno=True)
        native = ctypes.CDLL(None, use_errno=True)
        print(json.dumps({"source": SOURCE_URL, "source_sha256": SOURCE_SHA256,
                          "host": platform.platform(),
                          "scope": "Historical Apple function compiled on modern macOS; not a physical iPhone test",
                          "historical": probe(old.uf_historical_strerror_r),
                          "modern_host": probe(native.strerror_r)}, indent=2))


if __name__ == "__main__":
    main()
