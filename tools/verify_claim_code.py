#!/usr/bin/env python3
"""Derive a Little Printer's Zigbee link key from its claim code.

Useful when a printer joins the Bridge but never prints: compare the key derived
here against the one your server actually pushes (visible in the Bridge's daemon
log as `add_device_encryption_key`). A mismatch means the server holds a stale
claim code, usually because the printer was claimed before being factory reset.

Self-tests against Sirius's own published vector before trusting its output.

Needs only python3 and the `openssl` binary. No third-party packages.

Usage:
    verify_claim_code.py CLAIM_CODE [DEVICE_ADDRESS_HEX] [SERVER_KEY]

    CLAIM_CODE          e.g. 1234-abcd-5678-efgh
    DEVICE_ADDRESS_HEX  the printer's 8-byte EUI64, e.g. 000d6f00xxxxxxxx.
                        Appears in the Bridge log as `device_address`.
    SERVER_KEY          what the server is pushing, hex or base64.

Derivation ported from nordprojects/sirius (sirius/coding/claiming.py and
bitshuffle.py), reimplemented here so this file stands alone.
"""
import base64
import binascii
import struct
import subprocess
import sys

SALT = bytes([0x38, 0x96, 0x10, 0xD9, 0xB6, 0xB1, 0x0D, 0x16,
              0x9E, 0xE9, 0xBF, 0x87, 0x95, 0x32, 0x62, 0x5B])

# Base32 for claim codes. 'A' is deliberately absent. I and L map to 1, U to V,
# so that common misreadings still decode.
ALPHABET = {
    '0': 0x00, '1': 0x01, '2': 0x02, '3': 0x03, '4': 0x04, '5': 0x05,
    '6': 0x06, '7': 0x07, '8': 0x08, '9': 0x09, 'B': 0x0A, 'C': 0x0B,
    'D': 0x0C, 'E': 0x0D, 'F': 0x0E, 'G': 0x0F, 'H': 0x10, 'I': 0x01,
    'J': 0x11, 'K': 0x12, 'L': 0x01, 'M': 0x13, 'N': 0x14, 'O': 0x15,
    'P': 0x16, 'Q': 0x17, 'R': 0x18, 'S': 0x19, 'T': 0x1A, 'U': 0x1B,
    'V': 0x1B, 'W': 0x1C, 'X': 0x1D, 'Y': 0x1E, 'Z': 0x1F,
}


def crc16_ccitt(data, crc=0xFFFF):
    """CRC-16/CCITT-FALSE, table-free so this file has no data blob."""
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def aes128_ecb_encrypt(key, block):
    result = subprocess.run(
        ['openssl', 'enc', '-aes-128-ecb', '-K', binascii.hexlify(key).decode(), '-nopad'],
        input=block, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)
    return result.stdout[:16]


def unpack(claim_code):
    """-> (device_xor, secret, crc, raw_80_bit_value)"""
    clean = claim_code.replace('-', '').upper()
    if len(clean) != 16:
        raise ValueError('claim codes are 16 characters, got %d' % len(clean))
    value = 0
    for i, char in enumerate(clean):
        if char not in ALPHABET:
            raise ValueError('%r is not a valid claim-code character' % char)
        value += ALPHABET[char] * (32 ** (15 - i))
    return value & 0xFFFFFF, (value >> 24) & 0xFFFFFFFFFF, value >> 64, value


def link_key(secret_5_bytes):
    """Two AES-128-ECB rounds over the salt and the padded secret."""
    block = secret_5_bytes + b'\x80' + b'\x00' * (16 - 2 - len(secret_5_bytes) - 1)
    bit_length = (len(secret_5_bytes) + 16) * 8
    block += bytes([bit_length >> 8 & 0xFF, bit_length & 0xFF])
    out = bytes(16)
    for chunk in (SALT, block):
        encrypted = aes128_ecb_encrypt(out, chunk)
        out = bytes(encrypted[i] ^ chunk[i] for i in range(16))
    return out


def process(claim_code):
    """-> (device_xor, link_key_bytes, crc_is_valid)"""
    device_xor, secret, crc, raw = unpack(claim_code)
    crc_ok = crc16_ccitt(struct.pack('<Q', raw & 0xFFFFFFFFFFFFFFFF)) == crc
    return device_xor, link_key(struct.pack('<Q', secret)[0:5]), crc_ok


def hardware_xor(device_address_hex):
    """The 3-byte device field a claim code carries, from an 8-byte EUI64.

    Note the byte reversal. notjosh/little-printer-claim-code omits it and does
    not reproduce Sirius's published vector.
    """
    b = bytearray.fromhex(device_address_hex)
    if len(b) != 8:
        raise ValueError('device address should be 8 bytes, got %d' % len(b))
    b.reverse()
    c = bytearray(3)
    c[0] = b[0] ^ b[5]
    c[1] = b[1] ^ b[3] ^ b[6]
    c[2] = b[2] ^ b[4] ^ b[7]
    return c[2] << 16 | c[1] << 8 | c[0]


def self_test():
    _, key, crc_ok = process('6xwh-441j-8115-zyrh')
    assert crc_ok, 'self-test: CRC failed'
    assert base64.b64encode(key) == b'F7D9bmztHV32+WJScGZR0g==', 'self-test: wrong key'


def main(argv):
    self_test()
    print("self-test: PASS (reproduces Sirius's published vector)")
    print()

    if len(argv) < 2:
        print(__doc__.split('Usage:')[1].rstrip())
        return 2

    claim_code = argv[1]
    device_xor, key, crc_ok = process(claim_code)
    key_hex = binascii.hexlify(key).decode()

    print('claim code       : %s' % claim_code)
    print('CRC valid        : %s' % ('yes' if crc_ok
                                     else 'NO - mistyped, or not a real claim code'))
    print('device field     : 0x%06x' % device_xor)

    if len(argv) > 2:
        expected = hardware_xor(argv[2])
        print('printer requires : 0x%06x  (from %s)' % (expected, argv[2]))
        print('  -> %s' % ('MATCH: this code belongs to this printer'
                           if device_xor == expected else
                           'MISMATCH: this code is not for this printer'))

    print()
    print('derived link key : %s' % key_hex)
    print('             b64 : %s' % base64.b64encode(key).decode())

    if len(argv) > 3:
        given = argv[3].strip()
        try:
            server_hex = binascii.hexlify(base64.b64decode(given, validate=True)).decode()
        except Exception:
            server_hex = given.lower()
        print('server pushes    : %s' % server_hex)
        print('  -> %s' % ('MATCH: the key is correct, look elsewhere'
                           if key_hex == server_hex else
                           'MISMATCH: the server holds a different claim code'))
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
