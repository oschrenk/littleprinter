"""Regenerate the fixtures and the golden vectors in this directory.

The golden .bin files are the output of Sirius's own encoder, so the Rust crate
is tested against the Python original rather than against itself.

    uv run --with pillow --with pycryptodome python generate.py <sirius-checkout>

<sirius-checkout> is a clone of https://github.com/nordprojects/sirius; the
vectors here were made at HEAD 9d88c63. Pillow and pycryptodome are needed
because sirius.coding.encoders imports claiming, which imports Crypto.Cipher.

Every fixture is saved as a mode "1" PNG. Sirius's convert_to_1bit is
im.convert('1'), which Pillow dithers Floyd-Steinberg; on an image that is
already 1-bit it is a no-op, so both encoders see identical pixels and the
byte-for-byte comparison is fair. The Rust encoder does not dither at all and
refuses input that is not already pure black and white.
"""

import base64
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ADDR = "0000000000000001"
# Deliberately asymmetric, so a byte-order mistake in print_id cannot hide.
CMD_ID = 0x12345678


def main(sirius):
    sys.path.insert(0, sirius)
    from PIL import Image
    from sirius.coding import encoders, image_encoding
    from sirius.protocol import messages

    fixtures = {
        "white1": Image.new("1", (384, 1), 1),
        "black1": Image.new("1", (384, 1), 0),
        "normalface": Image.open(os.path.join(sirius, "tests/normalface.png")).convert("1"),
        "barcodehead": Image.open(os.path.join(sirius, "tests/barcode-head.png")).convert("1"),
    }

    for name, im in fixtures.items():
        path = os.path.join(HERE, name + ".png")
        im.save(path)
        # Reload from disk so Rust and Python read the exact same bytes.
        fixtures[name] = Image.open(path)
        first = list(fixtures[name].transpose(Image.ROTATE_180).getdata())[0]
        print(
            "%-12s %s first_post_rotation=%s"
            % (name, fixtures[name].size, "WHITE" if first else "BLACK")
        )

    # A greyscale pixel, to prove the Rust encoder refuses what it cannot match.
    grey = Image.new("L", (384, 1), 255)
    grey.putpixel((7, 0), 128)
    grey.save(os.path.join(HERE, "grey.png"))

    def dump(name, command):
        d = encoders.encode_bridge_command(ADDR, command, CMD_ID, "2026-10-29T00:00:00")
        raw = base64.b64decode(d["binary_payload"])
        open(os.path.join(HERE, name + ".bin"), "wb").write(raw)
        print("%-28s %6d bytes" % (name, len(raw)))

    for name, im in fixtures.items():
        dump("sdap_" + name, messages.SetDeliveryAndPrint(device_address=ADDR, pixels=im))

    dump(
        "personality",
        messages.SetPersonality(
            device_address=ADDR,
            face_pixels=fixtures["normalface"],
            nothing_to_print_pixels=fixtures["barcodehead"],
            cannot_see_bridge_pixels=fixtures["white1"],
            cannot_see_internet_pixels=fixtures["black1"],
        ),
    )

    # The hand-derived vector from LAB-154: the all-white 384x1 pixel block is
    # exactly 37 bytes. Asserted as a literal in the Rust tests too, so the
    # suite keeps one check that does not depend on Python at all.
    count, rle = image_encoding.rle_from_bw(fixtures["white1"])
    block = encoders._encode_pixels(count, rle)
    assert len(block) == 37, len(block)
    print("white1 pixel block: %d bytes  %s" % (len(block), block.hex()))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(os.path.abspath(sys.argv[1]))
