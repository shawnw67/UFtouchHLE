#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Generate comparison fixtures from an externally supplied GNU source file.

Download SOURCE_URL separately, preserving its LGPL header. The reference is
compiled only in a temporary directory and never linked into touchHLE.
Usage: python3 dev-scripts/strverscmp-oracle.py --reference SOURCE.c --output FILE
"""
import argparse
import ctypes
import hashlib
import itertools
import json
import locale
import pathlib
import random
import subprocess
import tempfile

SOURCE_URL = "https://raw.githubusercontent.com/bminor/glibc/66f3e9219d8f86b977d9be04ad469b5d72af0da2/string/strverscmp.c"
SOURCE_SHA256 = "03dde07efc33255d921adbe4ce27a70aa68d5f0c397cc12398daf8d4ace3e9d6"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    reference = args.reference.resolve()
    if hashlib.sha256(reference.read_bytes()).hexdigest() != SOURCE_SHA256:
        parser.error("Reference source differs from the pinned GNU version")
    locale.setlocale(locale.LC_CTYPE, "C")
    with tempfile.TemporaryDirectory(prefix="uf-version-oracle-") as directory:
        directory = pathlib.Path(directory)
        wrapper = directory / "wrapper.c"
        wrapper.write_text(
            "#define __strverscmp uf_oracle_strverscmp\n"
            "#define libc_hidden_def(name)\n#define weak_alias(from, to)\n"
            "#include " + json.dumps(str(reference)) + "\n"
        )
        library = directory / "oracle.dylib"
        subprocess.run(["clang", "-shared", "-fPIC", str(wrapper), "-o", str(library)], check=True)
        oracle = ctypes.CDLL(str(library)).uf_oracle_strverscmp
        oracle.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
        oracle.restype = ctypes.c_int
        edges = [b"", b"0", b"00", b"000", b"01", b"010", b"09", b"1", b"9",
                 b"10", b"12", b"1a", b"a1", b"0/", b"0.", b"a", b"A",
                 b"\xff", b"\x80", b"v1.09", b"v1.010"]
        pairs = list(itertools.product(edges, repeat=2))
        rng = random.Random(0x5546484c)
        alphabet = b"/019Az.-\x80\xff"
        for _ in range(1024):
            prefix = bytes(rng.choice(alphabet) for _ in range(rng.randrange(9)))
            pairs.append(tuple(prefix + bytes(rng.choice(alphabet) for _ in range(rng.randrange(25))) for _ in range(2)))
        pairs.extend([(b"9" * 128, b"1" * 129), (b"1" * 256, b"9" * 255)])
        lines = ["# GNU source: " + SOURCE_URL, "# SHA-256: " + SOURCE_SHA256,
                 "# Hex byte strings; comparison sign only. '-' means empty."]
        for a, b in pairs:
            value = oracle(a, b)
            sign = (value > 0) - (value < 0)
            lines.append(f"{a.hex() or '-'}\t{b.hex() or '-'}\t{sign}")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text("\n".join(lines) + "\n")
        print(f"Generated {len(pairs)} GNU oracle cases")


if __name__ == "__main__":
    main()
