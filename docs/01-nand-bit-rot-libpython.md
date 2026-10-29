# A dead NAND page silently zeroed 2 KB of libpython

The headline finding. If you take one thing from this repo, take this.

## Symptoms

Three failures that look completely unrelated:

- The Zigbee radio appears dead. `import ezsp` works, but **any** object construction
  segfaults: `ezsp.EmberApsFrame()`, `EmberNetworkParameters()`, `EmberKeyData()`.
- `import bottle` segfaults, so `status_server` cannot load, so the port 81 configuration UI
  never appears.
- `dns.resolver.Resolver()` segfaults, so the cloud client cannot resolve anything.

The daemon therefore never starts, and because the Zigbee driver and the web UI live in the
same process, the printer cannot find the Bridge *and* you have no configuration page. Two
symptoms, one cause.

## The actual cause

A single NAND page lost charge during years in storage. Its ECC could not correct it, so
jffs2 discarded that data node — and **jffs2 returns zeros for a discarded node, with no
error to userspace**.

That node held 2048 bytes of `/usr/lib/libpython2.7.so.1.0` at file offset
`0x08a800`–`0x08b000`, which contains CPython's **`slot_tp_setattro`**: the function that
dispatches `__setattr__`.

ARM executes `00000000` as `andeq r0, r0, r0`, effectively a no-op. So a call to
`slot_tp_setattro` NOPs through the dead region, falls into whatever follows, and re-enters
the setattr path. That is unbounded C recursion which creates **no Python frames** — so
`sys.setrecursionlimit()` never fires — until the 8 MB stack is exhausted and the process
takes SIGSEGV.

## The minimal reproduction

Nothing BERG-specific. Run this on your Bridge:

```python
class Plain(object):
    pass
p = Plain(); p.x = 1          # fine

class Custom(object):
    def __setattr__(self, name, value):
        object.__setattr__(self, name, value)
c = Custom()
c.x = 1                        # SIGSEGV
```

**Any class defining `__setattr__` in Python segfaults the interpreter.** That is rare enough
that everything else on the device works perfectly, which is exactly why it is so hard to
find. SWIG proxy classes, dnspython's `Name`, and several of bottle's classes all define it.

## Why every integrity check missed it

This is the part worth internalising.

- **`md5sum` cannot detect it.** The read succeeds, raises nothing, and is byte-stable across
  repeated reads. It is simply zeros where code should be.
- **A `cat` of every file on the filesystem reported zero failures**, for the same reason.
- **A recursive checksum against a known-good copy only validates files that have one.**
  Several hundred files matched upstream; `libpython` had no reference, so it was never
  checked.

If a test cannot produce evidence of the failure you are hunting, a clean result from it
proves nothing. Write down what each check cannot see.

## What *did* point at it

`dmesg`, on **every single boot**, from the very first one:

```
uncorrectable error : mtd->read(0x268 bytes from 0x88bd98) returned ECC error
JFFS2 notice: check_node_data: wrong data CRC in data node at 0x0088bd98
```

I dismissed that twice as obsolete-node garbage, with a confident-sounding justification.
**It was the answer the whole time.** If you see those lines, chase the address.

## Confirming it from the raw flash

Dump the partition and scan it (see `tools/jffs2_scan.py` and `tools/jffs2_extract.py`):

```sh
# on the Bridge. /dev/mtd6 may not exist as a node - see docs/05
nanddump -a -q --bb=padbad /dev/mtd6 > mtd6-root.img
```

For my unit, of **2260** jffs2 data nodes belonging to libpython's inode, **2259 had valid
data CRCs and exactly one did not** — and that one covered precisely the zeroed range, at
flash offset `0x88bd98`, the address dmesg had been printing all along.

The node is **not recoverable from the flash**: its zlib payload fails to inflate with
`Error -3: invalid bit length repeat`. The compressed bytes are too damaged.

Find zero runs in the file directly:

```python
d = open('libpython2.7.so.1.0','rb').read()
runs, i = [], 0
while i < len(d):
    if d[i] == 0:
        j = i
        while j < len(d) and d[j] == 0: j += 1
        if j - i >= 64: runs.append((hex(i), j - i))
        i = j
    else:
        i += 1
print(runs)
```

Ordinary padding appears as scattered 64–300 byte runs in data and string sections. A
**2048-byte, 0x800-aligned run inside `.text`** is not padding.

## The fix

**`kpeeem/berg-bridge-dump` contains `usr/lib/libpython2.7.so.1.0`.** That repo is a full
filesystem dump, not just the daemon directory. It is the same build.

A byte comparison against my damaged copy gave **1834 differing bytes, every one inside
`0x08a800`–`0x08affe`, and zero differences anywhere else** in the 4.6 MB file.

Splice in only the damaged range, so nothing else can change:

```python
dump = open('dump/usr/lib/libpython2.7.so.1.0','rb').read()
dev  = bytearray(open('device/libpython2.7.so.1.0','rb').read())
dev[0x8a800:0x8b000] = dump[0x8a800:0x8b000]
open('repaired','wb').write(bytes(dev))
```

Verify before installing. `slot_tp_setattro` should disassemble into real ARM, ending where
CPython's source says:

```
8aa60: e88d5000   stm  sp, {r12, lr}
8aa64: ebffff57   bl   0x8a7c8 <call_method>
8aa68: e3500000   cmp  r0, #0
8aa6c: 03e00000   mvneq r0, #0
```

Install with the daemon stopped (it has the library mmap'd), write to a temp name, assert the
checksum, then `mv`. Keep the damaged original.

## How it was actually diagnosed

In case you need to do something similar, the ladder that worked, after a long detour through
hypotheses that did not:

1. **Core dump, analysed elsewhere.** `ulimit -c unlimited` then reproduce. The kernel writes
   a core with no tooling needed on the device.
2. **Parse `NT_PRSTATUS` from the core** for the registers. On ARM, `pr_reg` sits at offset 72
   in `elf_prstatus`, and the PC is index 15 of 18 words.
3. **Resolve the PC** against `/proc/self/maps` captured in the *same* process, then to a
   symbol with `llvm-nm` on a workstation.
4. **`sys.settrace` logging only `call` events**, unbuffered to a file, to find the last
   function entered before a crash that prints nothing.
5. **Bisect by running each construction step in its own process**, so one segfault cannot
   mask the next.
6. **Hexdump the resolved symbol.** That is where the zeros were.

A caution on step 3: do **not** trust `llvm-objdump --triple=armv5te` for integrity scanning.
It fails to decode ARM multiply encodings and reports roughly 5% of all instructions as
`<unknown>`, which looks alarming and means nothing.
