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
