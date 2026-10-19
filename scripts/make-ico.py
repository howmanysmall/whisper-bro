"""Pack PNG files into a Windows .ico with PNG-compressed entries.

Usage: python3 -I scripts/make-ico.py <png...> <out.ico>
"""

import struct
import sys


def png_size(data):
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG file")
    return struct.unpack(">II", data[16:24])


def main(argv):
    if len(argv) < 3:
        sys.exit("usage: make-ico.py <png...> <out.ico>")
    images = []
    for path in argv[1:-1]:
        with open(path, "rb") as handle:
            data = handle.read()
        width, height = png_size(data)
        images.append((width, height, data))
    out = bytearray(struct.pack("<HHH", 0, 1, len(images)))
    offset = 6 + 16 * len(images)
    for width, height, data in images:
        # A stored size of 0 means 256.
        out += struct.pack(
            "<BBBBHHII",
            0 if width >= 256 else width,
            0 if height >= 256 else height,
            0,
            0,
            1,
            32,
            len(data),
            offset,
        )
        offset += len(data)
    for _, _, data in images:
        out += data
    with open(argv[-1], "wb") as handle:
        handle.write(out)


main(sys.argv)
