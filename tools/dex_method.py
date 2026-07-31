#!/usr/bin/env python3
"""Look up method signatures in a dex file.

Every Java method Cordial implements has to match Roblox's descriptor exactly —
libjnivm binds hooks by descriptor, and a mismatch is silent (the hook registers
and is simply never called). Guessing signatures from the AGDK source is close
enough to be dangerous, because they change between versions and Roblox ships
whichever it built against.

`apktool`/`baksmali` need a JVM, which the target environment does not have.
Dex is a simple enough format to read directly for this one question.

Usage:
    tools/dex_method.py <dir-of-dex-or-dex-file> initializeNativeCode
    tools/dex_method.py apk/dex/ --class com/google/androidgamesdk/GameActivity
"""

import argparse
import pathlib
import struct
import sys

HEADER = {
    "string_ids": 0x38,
    "type_ids": 0x40,
    "proto_ids": 0x48,
    "method_ids": 0x58,
}


def uleb128(buf, off):
    result, shift = 0, 0
    while True:
        byte = buf[off]
        off += 1
        result |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return result, off
        shift += 7


class Dex:
    def __init__(self, data):
        if data[:4] != b"dex\n":
            raise ValueError("not a dex file")
        self.data = data
        self.sizes, self.offsets = {}, {}
        for name, pos in HEADER.items():
            size, offset = struct.unpack_from("<II", data, pos)
            self.sizes[name], self.offsets[name] = size, offset

    def string(self, idx):
        off = struct.unpack_from("<I", self.data, self.offsets["string_ids"] + idx * 4)[0]
        length, off = uleb128(self.data, off)
        end = self.data.index(b"\x00", off)
        return self.data[off:end].decode("utf-8", "replace")

    def type_name(self, idx):
        descriptor_idx = struct.unpack_from(
            "<I", self.data, self.offsets["type_ids"] + idx * 4
        )[0]
        return self.string(descriptor_idx)

    def proto(self, idx):
        """Return the JNI descriptor, e.g. `(Ljava/lang/String;I)J`."""
        base = self.offsets["proto_ids"] + idx * 12
        _shorty, return_idx, params_off = struct.unpack_from("<III", self.data, base)
        params = []
        if params_off:
            count = struct.unpack_from("<I", self.data, params_off)[0]
            for i in range(count):
                type_idx = struct.unpack_from("<H", self.data, params_off + 4 + i * 2)[0]
                params.append(self.type_name(type_idx))
        return f"({''.join(params)}){self.type_name(return_idx)}"

    def methods(self):
        for i in range(self.sizes["method_ids"]):
            base = self.offsets["method_ids"] + i * 8
            class_idx, proto_idx, name_idx = struct.unpack_from("<HHI", self.data, base)
            yield (
                self.type_name(class_idx),
                self.string(name_idx),
                self.proto(proto_idx),
            )


def dex_files(target):
    path = pathlib.Path(target)
    if path.is_dir():
        return sorted(path.glob("*.dex"))
    return [path]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("target", help="a .dex file or a directory of them")
    ap.add_argument("name", nargs="?", help="method name to look for")
    ap.add_argument("--class", dest="klass", help="restrict to this class (slashed form)")
    args = ap.parse_args()

    if not args.name and not args.klass:
        print("give a method name, a --class, or both", file=sys.stderr)
        return 2

    hits = 0
    for path in dex_files(args.target):
        dex = Dex(path.read_bytes())
        for klass, name, descriptor in dex.methods():
            if args.name and name != args.name:
                continue
            if args.klass and args.klass not in klass:
                continue
            print(f"{klass[1:-1]}.{name}{descriptor}")
            hits += 1

    if not hits:
        print("no match", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
