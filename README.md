# Little Printer revival notes

Field notes from reviving a **BERG Little Printer** and its **Berg Cloud Bridge** in 2026,
eleven years after BERG shut down. Written because every one of these cost hours, and none of
them is in the existing guides.

The hardware was fine. Everything that failed, failed for a reason nobody had documented.

## The findings

| # | Finding | Why it matters |
| --- | --- | --- |
| 1 | [A dead NAND page silently zeroed 2 KB of `libpython`](docs/01-nand-bit-rot-libpython.md) | **Read this one.** jffs2 returns zeros for a discarded node with no error, so `md5sum` cannot detect it. Took out three unrelated-looking subsystems. |
| 2 | [The Bridge picks a Zigbee channel the printer never scans](docs/02-zigbee-channel-selection.md) | A real bug in BERG's own code. The printer can never find the Bridge. |
| 3 | [Claim codes, link keys, and why a reset printer stops working](docs/03-claim-codes-and-link-keys.md) | A printer claimed *before* a factory reset will join and rejoin forever. Includes how to verify a link key yourself. |
| 4 | [Serial console and getting root](docs/04-serial-console-and-root.md) | The published pinout is reversed, `single` does nothing, and the password is DES-truncated to 8 characters. |
| 5 | [Assorted Bridge gotchas](docs/05-bridge-gotchas.md) | The boot loop, the disabled SSH stub, the watchdog that reboots you mid-debug, the dead update server costing 30 s per boot, and why a restart voids the printer pairing. |

## Tools

In [`tools/`](tools/), all dependency-free Python 3:

- `verify_claim_code.py` — derive a printer's link key from its claim code and check it
  against what your server is actually pushing. Validates itself against Sirius's published
  test vector before trusting its own output.
- `jffs2_scan.py` — list jffs2 directory entries and inode versions in a raw NAND image.
  Finds every historical version of a file, including deleted ones.
- `jffs2_extract.py` — reassemble a file from jffs2 data nodes in a raw image, reporting
  which nodes failed their CRC and which byte ranges are therefore missing.

## The client

[`client/`](client/) is a Rust reimplementation of the Sirius server's image-to-payload encoder,
so a payload can be built locally instead of only on the server. The byte layout it implements
is documented nowhere public; it was read out of `nordprojects/sirius` and is written up in the
source.

It is verified **byte-identical** to Sirius's own Python output, using Sirius's committed
`barcode-head.png` as the fixture. One check is a hand-derived 37-byte vector asserted as a
literal, so the suite does not depend on having Python and Pillow available.

It deliberately **differs** from Python for one class of image, and that difference is the
interesting part. `rle_from_bw` tries to guarantee the encoded stream starts with a white run:

```python
WHITE = '\xff'
BLACK = '\x00'
if groups[0][0] == BLACK:
    groups.insert(0, (WHITE, 0))
```

`getdata()` on a mode `"1"` image yields **ints**, 0 or 255, while `BLACK` is a one-character
**str**. On Python 3 that is never equal, so the guard never fires. The consequence is sharper
than a missing edge case: **an all-black row encodes byte-identically to an all-white row** —
same length, same md5. Two opposite images are indistinguishable on the wire, and the printer
resolves the ambiguity by inverting.

So if the first pixel after the encoder's 180° rotation is black — which is the artwork's
**bottom-right** pixel — the print comes out inverted. Note that BERG's own `normalface.png`,
committed in `nordprojects/sirius`, is black-first. **Their own face asset prints inverted
through their own encoder.** This client inserts the zero-length white run, so it is one byte
longer than Python for such images, and a test asserts that divergence on purpose.

Measured on the Bridge itself: 455 kB static `armv5te-unknown-linux-musleabi` binary, 848 kB
peak resident for a four-image `SetPersonality`, 269 ms. Output is byte-identical between the
macOS and ARM builds.

```sh
nix develop      # or direnv, via .envrc
task test        # 7 tests, against the golden vectors
task bridge      # static ARM build, checked with file(1)
```

One deliberate limitation: it **refuses any pixel that is not pure black or white**, rather than
guessing at Pillow's Floyd–Steinberg dither. Artwork must be 1-bit before it reaches the encoder.

## If your Bridge is misbehaving, check these first

1. **`dmesg | grep -i "uncorrectable\|wrong data CRC"`.** If anything appears, you may have
   silent data loss. A checksum will not find it. See finding 1.
2. **Is the daemon even running?** `ps | grep weminuche`. If not, nothing else matters.
   Note it only stays alive with `--daemon`; without it you get an interactive console that
   exits on EOF.
3. **Is it on a channel the printer scans?** The printer scans `11, 14, 15, 19, 20, 24, 25`.
   See finding 2.
4. **Is the server pushing the right link key?** Watch for `add_device_encryption_key` in the
   daemon log and verify it with `tools/verify_claim_code.py`. See finding 3.
5. **`--logfile` is off by default**, so the daemon keeps no log at all. Add it to
   `PY_DAEMON_OPTS` in `/usr/bin/oneshot_bergcloud_bridge.sh`. **And it takes a numeric
   level** — `--logfile-level INFO` crashes the daemon silently, because the code calls
   `int()` on it.

## Status of the ecosystem, as of October 2026

- `littleprinter.nordprojects.co` — **up**, and the only practical server. Twitter/X sign-in
  works. Has bugs; see finding 3.
- `alpha.littleprinter.com` — resolves, answers nothing. The claim page still tells you to
  point your Bridge at it. **Do not.**
- `bergcloud.com` and `bridge-updates.bergcloud.com` — gone. The latter still resolves, so
  you pay a full 30-second TCP timeout on every single boot. See finding 5.
- BERG's own help pages — gone, and the Internet Archive copy of `bergcloud.com/help/`
  returns an empty document.
- `tinyprinter.club` — up, but its Discord invite is expired.
- `kpeeem/berg-bridge-dump` — **the most useful artifact that still exists.** It is a full
  filesystem dump, not just the daemon. If a file on your Bridge is damaged, look there
  first; mine was.

## Credit

The community did the hard archaeology years earlier: `nordprojects/sirius` and
`genmon/sirius` for the server, `kpeeem/berg-bridge-dump` for the filesystem dump,
`notjosh/little-printer-claim-code` for the claim-code port, pipt and joerick for the
original rooting write-ups, and `wiki.bytewerk.org` for independent hardware notes. These
notes only add what broke after a decade in storage.
