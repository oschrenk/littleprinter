# Tools

Python 3, no third-party packages. `verify_claim_code.py` shells out to `openssl` for AES;
the others need only the standard library.

## verify_claim_code.py

Derive a printer's Zigbee link key from its claim code, and optionally check it against what
your server is actually pushing.

```sh
./verify_claim_code.py 1234-abcd-5678-efgh 000d6f00xxxxxxxx <server-key>
```

It self-tests against Sirius's published vector before printing anything, so a `PASS` line
means the implementation is sound on your machine.

Use it when the printer joins the Bridge but never prints. Watch the daemon log for
`add_device_encryption_key`, take the `encryption_key` value, and compare. A mismatch means
the server holds a stale claim code — see `../docs/03-claim-codes-and-link-keys.md`.

Two things it checks that are easy to get wrong by hand:

- The **CRC**, which tells you whether you transcribed the code correctly off thermal paper.
- The **device field**, against your printer's EUI64, using Sirius's byte-reversing
  `hardware_xor_from_device_address`. The npm port omits that reversal.

## jffs2_scan.py

List jffs2 directory entries and inode versions in a raw NAND image. Finds historical
versions of a file, including deleted ones, because jffs2 does not erase overwritten data
immediately.

```sh
./jffs2_scan.py mtd6-root.img libpython2.7.so.1.0
```

An entry with `ino=0` is a deletion. More than one entry for a name means an older version of
that file may still be sitting in the image.

## jffs2_extract.py

Reassemble a file from its data nodes, reporting which nodes failed their CRC and exactly
which byte ranges are therefore missing.

```sh
./jffs2_extract.py mtd6-root.img 630 recovered.so
```

This is how you find silent data loss. jffs2 discards a node whose CRC fails and returns
**zeros** for that range with no error to userspace, so a checksum of the live file cannot
see it.

### Worked example

Run against the image from the device these notes came from:

```
$ ./jffs2_scan.py mtd6-root.img libpython2.7.so.1.0
dirent nodes            : 2146
inodes with data nodes  : 2042

dirent entries named 'libpython2.7.so.1.0': 1
   ino=630      dirent_version=628   parent=475      data_nodes=2260

$ ./jffs2_extract.py mtd6-root.img 630 recovered.so
data nodes     : 2260  (crc ok 2259, crc bad 1)
compression    : ['none', 'zlib']
declared size  : 4594655 bytes

problem nodes:
  v280   file range 0x08a800-0x08b000  flash offset 0x88bd98  decompression failed: Error -3 while decompressing data: invalid bit length repeat

unwritten bytes: 2048 of 4594655
  hole 0x08a800-0x08b000  (2048 bytes)

wrote recovered.so (4594655 bytes), first 4 bytes b'\x7fELF'
```

One bad node out of 2260. Its flash offset, `0x88bd98`, is exactly the address the kernel had
been printing in `dmesg` on every boot for a decade. The 2048-byte hole it leaves sits inside
`.text` and contains CPython's `slot_tp_setattro`. See
`../docs/01-nand-bit-rot-libpython.md`.

### A gotcha if you write your own

**jffs2 uses the kernel's CRC-32 with seed 0 and no pre/post inversion.** In Python that is:

```python
zlib.crc32(data, 0xFFFFFFFF) ^ 0xFFFFFFFF
```

not `zlib.crc32(data, 0)`. Get it wrong and every single node looks corrupt, which is a very
convincing way to waste an hour.
