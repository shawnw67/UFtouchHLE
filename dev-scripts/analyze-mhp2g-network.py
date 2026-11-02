#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Read-only, stdlib-only ARMv7 Mach-O/IPA network inventory; JSON to stdout.

Does not launch the app, contact endpoints, extract files, or patch code.
Static references show presence, not execution or multiplayer criticality.
Supports little-endian 32-bit Objective-C 2 images, including ARMv7 fat slices.
Rejects encrypted slices and unsupported relative method lists explicitly.
Example: python3 dev-scripts/analyze-mhp2g-network.py Podroid_apps/p2g.ipa
"""

import argparse
import hashlib
import json
import plistlib
import re
import struct
import subprocess
import sys
import zipfile
from pathlib import Path


NETWORK = re.compile(
    r"GK|GameKit|GameCenter|Match|Session|Peer|[Nn]etwork|[Ss]ocket|[Rr]eachab|"
    r"[Bb]onjour|DNSService|NetService|Nakamap|Lobi|[Aa]uthentic|[Cc]onnect|"
    r"[Dd]isconnect|[Ss]end[Dd]ata|[Rr]eceive[Dd]ata|[Pp]layerID|[Pp]layers"
)
C_NETWORK = re.compile(
    r"^_(?:socket|bind|listen|accept|connect|send|sendto|recv|recvfrom|select|poll|"
    r"shutdown|getsock\w*|setsockopt|ioctl|fcntl|getifaddrs|freeifaddrs|if_\w*|"
    r"getaddrinfo|freeaddrinfo|gethost\w*|inet_\w*|DNSService\w*|CFSocket\w*|"
    r"CF\w*Stream\w*|SCNetwork\w*|dispatch_\w+|CC\w+|Sec\w+)$"
)


class MachO:
    def __init__(self, data):
        self.slices = []
        if data[:4] in (b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"):
            endian = ">" if data[:4] == b"\xca\xfe\xba\xbe" else "<"
            count = struct.unpack_from(endian + "I", data, 4)[0]
            for i in range(count):
                cpu, sub, offset, size, align = struct.unpack_from(endian + "5I", data, 8 + 20 * i)
                if offset + size > len(data):
                    raise ValueError("Fat slice outside file")
                self.slices.append(dict(cpu=cpu, subtype=sub, offset=offset, size=size))
            candidates = [s for s in self.slices if s["cpu"] == 12 and s["subtype"] & 0xFFFFFF == 9]
            if not candidates:
                raise ValueError("No ARMv7 slice (arm64 is not supported by this analyzer)")
            s = candidates[0]
            data = data[s["offset"]:s["offset"] + s["size"]]
        if data[:4] != b"\xce\xfa\xed\xfe":
            raise ValueError("Expected little-endian 32-bit Mach-O")
        self.data = data
        self.sections = []
        self.dylibs = []
        self.encryption = []
        self.symtab = None
        self.cpu, self.subtype = self.unpack("II", 4)
        commands, command_bytes = self.unpack("II", 16)
        if 28 + command_bytes > len(data):
            raise ValueError("Truncated load commands")
        offset = 28
        for _ in range(commands):
            cmd, size = self.unpack("II", offset)
            if size < 8 or offset + size > 28 + command_bytes:
                raise ValueError("Invalid load command")
            if cmd == 1:  # LC_SEGMENT
                nsects = self.unpack("I", offset + 48)[0]
                for i in range(nsects):
                    pos = offset + 56 + i * 68
                    if pos + 68 > offset + size:
                        raise ValueError("Section outside load command")
                    name, segment = self.unpack("16s16s", pos)
                    addr, length, fileoff = self.unpack("III", pos + 32)
                    self.sections.append(dict(name=name.rstrip(b"\0").decode(),
                                              segment=segment.rstrip(b"\0").decode(),
                                              address=addr, size=length, offset=fileoff))
            elif cmd in (12, 0x80000018, 0x8000001F, 0x20, 0x80000023):
                nameoff = self.unpack("I", offset + 8)[0]
                self.dylibs.append(dict(command=hex(cmd), path=self.string_at(offset + nameoff)))
            elif cmd == 2:
                self.symtab = self.unpack("IIII", offset + 8)
            elif cmd == 0x21:
                cryptoff, cryptsize, cryptid = self.unpack("III", offset + 8)
                self.encryption.append(dict(offset=cryptoff, size=cryptsize, cryptid=cryptid))
            offset += size
        if any(e["cryptid"] for e in self.encryption):
            raise ValueError("Encrypted slice: decrypted code is required for analysis")

    def unpack(self, fmt, offset):
        return struct.unpack_from("<" + fmt, self.data, offset)

    def string_at(self, offset):
        if not 0 <= offset < len(self.data):
            raise ValueError("String outside image")
        end = self.data.find(b"\0", offset)
        if end < 0:
            raise ValueError("Unterminated string")
        return self.data[offset:end].decode("utf-8", errors="replace")

    def fileoff(self, address):
        for section in self.sections:
            if section["address"] <= address < section["address"] + section["size"]:
                offset = section["offset"] + address - section["address"]
                if offset >= len(self.data):
                    break
                return offset
        raise ValueError("Unmapped address " + hex(address))

    def ptr(self, address):
        return self.unpack("I", self.fileoff(address))[0]

    def cstr(self, address):
        return self.string_at(self.fileoff(address)) if address else ""

    def section(self, name):
        return [s for s in self.sections if s["name"] == name]

    def pointers(self, name):
        for section in self.section(name):
            for offset in range(0, section["size"], 4):
                yield section["address"] + offset, self.unpack("I", section["offset"] + offset)[0]

    def methods(self, address):
        if not address:
            return []
        offset = self.fileoff(address)
        flags, count = self.unpack("II", offset)
        if flags & 0x80000000:
            raise ValueError("Unsupported relative Objective-C method list")
        stride = flags & 0xFFFF
        if stride < 12 or count > 100000:
            raise ValueError("Invalid method list")
        result = []
        for i in range(count):
            name, types, imp = self.unpack("III", offset + 8 + i * stride)
            result.append(dict(selector=self.cstr(name), types=self.cstr(types), imp=hex(imp)))
        return result

    def classes(self):
        result = []
        for _, addr in self.pointers("__objc_classlist"):
            isa = self.ptr(addr)
            ro = self.ptr(addr + 16) & ~3
            meta_ro = self.ptr(isa + 16) & ~3
            result.append(dict(name=self.cstr(self.ptr(ro + 16)), address=hex(addr),
                               protocols=self.protocol_names(self.ptr(ro + 24)),
                               instance_methods=self.methods(self.ptr(ro + 20)),
                               class_methods=self.methods(self.ptr(meta_ro + 20))))
        return result

    def protocol_names(self, address):
        if not address:
            return []
        count = self.ptr(address)
        if count > 10000:
            raise ValueError("Invalid protocol list")
        return [self.cstr(self.ptr(self.ptr(address + 4 + i * 4) + 4)) for i in range(count)]

    def imports(self):
        if self.symtab is None:
            return []
        offset, count, stroff, strsize = self.symtab
        result = []
        for i in range(count):
            stridx, kind, section, desc, value = self.unpack("IBBHI", offset + i * 12)
            if kind & 0xE == 0 and kind & 1 and stridx:
                if stridx >= strsize:
                    raise ValueError("Invalid symbol string index")
                name = self.string_at(stroff + stridx)
                ordinal = (desc >> 8) & 255
                lib = self.dylibs[ordinal - 1]["path"] if 1 <= ordinal <= len(self.dylibs) else None
                result.append(dict(name=name, library=lib, weak=bool(desc & 0x40)))
        return result

    def inventory(self):
        classes = self.classes()
        imports = self.imports()
        refs = [dict(address=hex(addr), selector=self.cstr(ptr))
                for addr, ptr in self.pointers("__objc_selrefs")]
        strings = []
        for section in self.section("__cstring"):
            payload = self.data[section["offset"]:section["offset"] + section["size"]]
            for match in re.finditer(rb"[^\0]+", payload):
                value = match[0].decode("utf-8", errors="replace")
                if NETWORK.search(value) or re.search(r"https?://|(?:[\w-]+\.)+(?:com|net|org|jp)(?:[/\s:]|$)", value):
                    strings.append(dict(address=hex(section["address"] + match.start()), text=value))
        return dict(
            slice_sha256=hashlib.sha256(self.data).hexdigest(), slice_bytes=len(self.data),
            cpu=self.cpu, subtype=self.subtype, slices=self.slices, encryption=self.encryption,
            dylibs=self.dylibs, imports=imports,
            network_imports=[s for s in imports if NETWORK.search(s["name"]) or C_NETWORK.search(s["name"])],
            class_names=[c["name"] for c in classes],
            protocol_names=[self.cstr(self.ptr(ptr + 4)) for _, ptr in self.pointers("__objc_protolist")],
            network_classes=[c for c in classes if NETWORK.search(c["name"])],
            network_methods=[dict(class_name=c["name"], kind=kind, **method)
                             for c in classes for kind in ("instance_methods", "class_methods")
                             for method in c[kind] if NETWORK.search(method["selector"])],
            selector_references=refs,
            network_selector_references=[s for s in refs if NETWORK.search(s["selector"])],
            network_strings=strings,
            counts=dict(classes=len(classes), imports=len(imports), selector_references=len(refs)),
            limitations=["Static presence is not proof of runtime execution.",
                         "Only main executable Objective-C classes/selector refs and C strings are inspected; no call graph or traffic capture.",
                         "Does not inspect resource archives or embedded binaries; absence cannot rule out dynamic lookups or custom protocols."])


def disassembly_evidence(image, executable, inventory):
    """Bounded Thumb disassembly with exact PC-relative selector-load matches.

    These are static references, not dynamic traces or a complete call graph.
    Only accepts MOVW/MOVT/ADD PC sequences within four instructions; records
    the raw instruction context for independent review.
    """
    refs = {int(s["address"], 16): s["selector"] for s in inventory["selector_references"]}
    selected = [c for c in inventory["network_classes"] if c["name"] in ("GameCenterSupport", "Reachability")]
    evidence = []
    for c in selected:
        for method in c["instance_methods"] + c["class_methods"]:
            start = int(method["imp"], 16) & ~1
            if not int(method["imp"], 16) & 1:
                continue
            # All classes, not just the selected ones, delimit each method.
            addresses = sorted(int(m["imp"], 16) & ~1 for item in image.classes()
                               for kind in ("instance_methods", "class_methods") for m in item[kind])
            stop = next((addr for addr in addresses if addr > start), start + 4)
            command = ["xcrun", "llvm-objdump", "-d", "--triple=thumbv7-apple-ios",
                       "--no-show-raw-insn", "--start-address=" + hex(start),
                       "--stop-address=" + hex(stop), str(executable)]
            output = subprocess.run(command, check=True, capture_output=True, text=True).stdout
            instructions = []
            for line in output.splitlines():
                match = re.match(r"\s*([0-9a-f]+):\s+(.*)", line)
                if match:
                    instructions.append((int(match[1], 16), match[2]))
            refs_found = []
            for i, (pc, instruction) in enumerate(instructions):
                add = re.fullmatch(r"add\s+(r\d+), pc", instruction)
                if not add:
                    continue
                register = add[1]
                nearby = instructions[max(0, i - 4):i]
                low = high = None
                for _, ins in nearby:
                    mov = re.fullmatch(r"mov([wt])\s+" + register + r", #(0x[0-9a-f]+|\d+)", ins)
                    if mov:
                        if mov[1] == "w":
                            low = int(mov[2], 0)
                            high = None
                        else:
                            high = int(mov[2], 0)
                if low is None or high is None:
                    continue
                address = pc + 4 + low + (high << 16)
                if address in refs:
                    refs_found.append(dict(pc=hex(pc), reference=hex(address), selector=refs[address],
                                           context=[hex(a) + ": " + ins for a, ins in instructions[max(0, i-4):i+6]]))
            if refs_found:
                evidence.append(dict(class_name=c["name"], selector=method["selector"],
                                     imp=method["imp"], selector_loads=refs_found))
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("--disassemble", type=Path, metavar="EXTRACTED_MACHO",
                        help="Optional macOS xcrun disassembly of an identical thin executable")
    args = parser.parse_args()
    metadata = {}
    if zipfile.is_zipfile(args.input):
        with zipfile.ZipFile(args.input) as archive:
            plists = [n for n in archive.namelist() if re.fullmatch(r"Payload/[^/]+\.app/Info\.plist", n)]
            if len(plists) != 1:
                raise ValueError("Expected exactly one top-level app bundle")
            info = plistlib.loads(archive.read(plists[0]))
            binary = plists[0].rsplit("/", 1)[0] + "/" + info["CFBundleExecutable"]
            data = archive.read(binary)
            metadata = {k: info.get(k) for k in (
                "CFBundleIdentifier", "CFBundleVersion", "CFBundleShortVersionString",
                "CFBundleExecutable", "MinimumOSVersion", "UIDeviceFamily", "UIRequiredDeviceCapabilities")}
            metadata["binary_member"] = binary
    else:
        data = args.input.read_bytes()
    image = MachO(data)
    result = image.inventory()
    if args.disassemble:
        if args.disassemble.read_bytes() != image.data:
            raise ValueError("Disassembly input differs from selected slice")
        result["disassembly_evidence"] = disassembly_evidence(image, args.disassemble, result)
    result["input"] = str(args.input)
    result["bundle"] = metadata
    result["executable_sha256"] = hashlib.sha256(data).hexdigest()
    json.dump(result, sys.stdout, ensure_ascii=False, indent=2)
    print()


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, struct.error, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        sys.exit("Analysis failed: " + str(error))
