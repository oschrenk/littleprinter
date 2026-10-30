# Assorted Bridge gotchas

Smaller things, each of which wasted time.

## The 60-second boot loop

`/usr/bin/oneshot_bergcloud_bridge.sh` ends with:

```sh
# We allow only so many restarts until we flat out reboot to fix any potential problems
echo "Rebooting!"
/sbin/reboot -f
```

The daemon cannot reach `bergcloud.com`, exits uncleanly, and after `RESTART_LIMIT` of 5 the
wrapper reboots the box. You get about 60 seconds of usable time, which is not enough to
diagnose anything.

**Comment out that final line** (line 138 on my unit). Keep the earlier `reboot -f` at line
119: that is the deliberate `EXIT_CODE -eq 99` path the daemon asks for.

```sh
sed -n '138p' /usr/bin/oneshot_bergcloud_bridge.sh   # confirm first
```

Note BusyBox `sed` has no usable `-i`; rebuild the file with `head`/`echo`/`tail` instead.

Community guides tell you to remove this line when installing `berg-bridge-dump`. Whoever
installed mine skipped it, which is where the loop came from. **Check it before assuming your
Bridge is broken.**

## SSH is disabled at the package level, not broken

`/etc/init.d/S60dropbear` ships as a deliberate stub:

```sh
#!/bin/sh
# SSH not enabled by package config
```

`dropbear` itself is installed and fine. `rcS` already runs every `/etc/init.d/S??*` in
numerical order, and `60` is after `S40network`, so you do not need to add a new script —
just replace the stub:

```sh
#!/bin/sh
case "$1" in
  start) dropbear ;;
  stop) killall dropbear 2>/dev/null ;;
esac
```

`chmod 755`, `sync`. Verify by breaking it: `stop` should close port 22, `start` reopen it.

## The watchdog reboots you mid-debug

The daemon's `--watchdog` option opens and pats `/dev/watchdog`. **Stop the daemon and the
kernel reboots the box about 16 seconds later.** This will repeatedly kill your SSH session
while you are trying to diagnose anything, and it looks like random instability.

Remove `--watchdog` from `PY_DAEMON_OPTS` while working:

```sh
PY_DAEMON_OPTS="--daemon --cloudlog --webdebug --logfile"
```

The log then says `Watchdog not enabled`, which is what you want. Put it back if you want
unattended recovery. Trade-off: without it, a genuinely wedged daemon stays wedged until you
power-cycle.

## `--daemon` is what keeps the daemon alive

`weminuche_bridge.pyc` does not fork. `--daemon` means "no interactive prompt", and the main
loop is literally:

```python
if self.options.daemon:
    while self.shutdown_triggered == False:
        time.sleep(1)
```

Run it **without** `--daemon` and it starts an interactive console instead. Over SSH, your
session closing sends EOF, which triggers `Shutdown called directly` and the daemon exits.
That looks like a crash and is not one.

In daemon mode it also replaces `sys.stdout`/`sys.stderr` with its own loggers, so a shell
redirect captures nothing. Use `--logfile` instead.

## Every boot wastes 30 seconds on a dead server

Before starting the daemon, the wrapper runs `updater.pyc`, which tries
`bridge-updates.bergcloud.com`. That host still resolves, to `23.23.232.155`, a recycled EC2
address. So DNS is fast and the SYN is black-holed, which is the worst combination: you pay a
full TCP timeout instead of a fast failure.

```
netstat: 192.168.x.x:59729  23.23.232.155:443  SYN_SENT
URLError: <urlopen error timed out>
```

The cost is **30 seconds**, not the 75 you might infer from how long a cold boot takes. The
timeout is a hardcoded constant in `updater.pyc`, in
`VerifiedHTTPSConnection.__init__`, passed to `socket.create_connection`:

```
CODE VerifiedHTTPSConnection
  CODE __init__   names=['httplib','HTTPSConnection','__init__','timeout']
    const int: 30
```

A ~73 second cold boot breaks down as 30 s of kernel and init, 35 s of wrapper, 5 s of Python
startup and imports, and 3 s of NCP reset to `EMBER_NETWORK_UP`. Of the wrapper's 35 s, 30 s
is this socket timeout, 2 s is a deliberate `sleep 2`, and the remaining 2.5 s is all of
`fw_printenv`, the two jffs2 remounts and an `ls` put together.

Measure it yourself without a stopwatch. Take field 22 of `/proc/<pid>/stat` for the wrapper
and for the daemon, subtract, and divide by `HZ`, which is 100 on this kernel:

```sh
ps w                             # find the wrapper and the python PIDs
cut -d' ' -f22 /proc/<pid>/stat
```

There is exactly one delay, not several. No NTP wait, no DNS wait, and no second network
attempt — because the sftp rescue path never runs. BERG shipped a bug: `main()` references
`fetch_sftp`, but the module only ever defines `fetch_ssh`. So after printing
`Import failed. Fetching SSH copy` the updater dies with `NameError`, which is the only
reason you pay 30 seconds and not 30 plus an SSH timeout.

### The edit

You cannot simply comment out the two `python` lines. They are the bodies of an
`if`/`elif` chain, and an empty branch body is a syntax error in `sh`. Falsify the two tests
instead, which adds and removes no lines, so nothing below moves:

```sh
  if false && [ -f $PY_UPDATE_SCRIPT ]; then       # was: if [ -f ... ]
    /usr/bin/python $PY_UPDATE_SCRIPT --outputfile $PACKAGE_UPDATE_FILE
  elif false && [ -f $PYC_UPDATE_SCRIPT ]; then    # was: elif [ -f ... ]
    /usr/bin/python $PYC_UPDATE_SCRIPT --outputfile $PACKAGE_UPDATE_FILE
  fi
```

On my unit those are lines 54 and 56. Only the `elif` actually fires in practice, because
`updater.py` does not exist and only `updater.pyc` does — but guard both, or you leave a trap
for the day something drops a `.py` into that directory.

The `.ipk` install loop that follows needs no change at all. It iterates
`$PACKAGE_DIRECTORY/*.ipk` over an empty `/usr/local/updates`, so it is already a no-op.

Edit `/usr/bin/oneshot_bergcloud_bridge.sh`, never the `/tmp` copy that `S99bergcloud_bridge`
makes. That copy is discarded each boot, and BusyBox `sh` reads a script incrementally, so
editing the file a running shell is still reading corrupts its control flow.

BusyBox `sed` has no usable `-i`. Write the result to a file in `/tmp`, check it, and only
then copy it over the original:

```sh
sed -e 's|^  if \[ -f \$PY_UPDATE_SCRIPT \]; then$|  if false \&\& [ -f $PY_UPDATE_SCRIPT ]; then|' \
    -e 's|^  elif \[ -f \$PYC_UPDATE_SCRIPT \]; then$|  elif false \&\& [ -f $PYC_UPDATE_SCRIPT ]; then|' \
    /usr/bin/oneshot_bergcloud_bridge.sh > /tmp/candidate.sh
sh -n /tmp/candidate.sh && echo OK          # do this BEFORE you reboot
mount -o remount,rw / && cp /tmp/candidate.sh /usr/bin/oneshot_bergcloud_bridge.sh
chmod 755 /usr/bin/oneshot_bergcloud_bridge.sh && sync && mount -o remount,ro /
```

**`sh -n` before rebooting is the check that matters.** It parses without executing, and it is
the only thing standing between a mangled quote and a Bridge that never starts, with no log
to say why. Note that `cp` does not preserve ownership: the stock file is owned by the uid
from BERG's build host, and yours will end up `root:root`. It runs as root, so this is
cosmetic.

### Nothing depends on the update check

Worth stating, because it is the question that decides whether the edit is safe. The updater
does not set the clock — `ntpd` is already up one second before the wrapper starts, and the
module references no time or NTP function at all. It does not touch the Zigbee NCP or its
firmware. It writes nothing: its only `open(..., 'wb')` sits behind a successful HTTP 200. The
`/usr/local/updates` directory it would write into is created higher up in the wrapper,
outside the edit. It cannot export anything, being a separate process. And the daemon has no
knowledge of it: `strings weminuche_bridge.pyc` grepped for `update` returns only `updateKey`
and `TCAddOrUpdateKey`, both Zigbee key management.

What you give up is the ability to receive an `ipk` from a company that has been gone for a
decade, and a rescue path that was already dead code.

**The real risk is the write, not the diff.** On a device this age, any jffs2 write rewrites
the inode and can wake garbage collection, which rewrites other blocks. Keep the original
file text somewhere off the device first; see `docs/01-nand-bit-rot-libpython.md` for what a
single lost page costs.

## The daemon dies with the ssh session that started it

`/etc/init.d/S99bergcloud_bridge` copies the wrapper to `/tmp` and backgrounds it with `&`.
Run from init at boot, that is fine. Run it over ssh and the wrapper lands in your session's
process group, so it takes `SIGHUP` when the session closes and the Python daemon goes with
it.

This is a nasty one to diagnose, because the restart looks successful and the failure appears
minutes later, when you next check:

```
ps | grep weminuche | grep -v grep   ->  nothing
netstat -tn                          ->  no connection to your server
cat /sys/class/gpio/gpio86/value     ->  1, Zigbee LED off
```

Start it detached instead:

```sh
cp /usr/bin/oneshot_bergcloud_bridge.sh /tmp/
chmod 755 /tmp/oneshot_bergcloud_bridge.sh
cd / && nohup /tmp/oneshot_bergcloud_bridge.sh >/dev/null 2>&1 &
```

## A Bridge restart voids the printer pairing

Every time the daemon starts it clears the key table, scans for an unused PAN ID, and forms a
brand new Zigbee network. The literals are visible in `strings api.pyc`, in this order:

```
Removing all link keys
scanForUnusedPanId
Forming network on channel %i, panID 0x%04x
emberFormNetwork
```

It never attempts to resume the network it had. Nothing is persisted locally to let it:
`/var/lib` is three symlinks into `/tmp`, which is tmpfs.

So the printer wakes to an unfamiliar PAN ID with no valid link key, and its only route back
is an unsecured join followed by a link key fetched from the server. Measured on my unit, that
took **three minutes 55 seconds across eleven join attempts**, each with a fresh short
address, before the first `DeviceEvent` heartbeat arrived. Signal was never the issue; RSSI
settled between -43 and -51 dBm.

**A red light in the few minutes after a Bridge restart is normal.** Wait it out before
concluding anything is broken. The extended PAN ID is worth a look while you do: mine was
`0x42455247143cf7ca`, and `0x42455247` is ASCII `BERG`. Only the low 32 bits are random.

## `/dev/mtd6` does not exist

The factory image creates device nodes for `mtd0` to `mtd5` and `mtdblock0` to `mtdblock5`
only. The kernel resolves `root=/dev/mtdblock6` internally without needing one, so nothing
notices until you try to dump the root partition.

Create it in RAM, not on the flash you are imaging:

```sh
mount -t tmpfs tmpfs /tmp
mknod /tmp/mtd6 c 90 12          # major 90, minor 2*partition
nanddump -a -q --bb=padbad /tmp/mtd6 > /tmp/root.img
```

**`--bb=padbad` is mandatory.** The `nanddump` default is `--bb=skipbad`, which omits bad
blocks and shifts every later offset, producing an image you cannot reflash.

Also: `nanddump` needs its absolute path `/usr/sbin/nanddump` over a non-interactive SSH
command, because that shell's `PATH` omits `/usr/sbin`.

## Watch out for tmpfs eating your RAM

The Bridge has **29 MB of RAM**, and `/tmp` is a tmpfs with a 14 MB cap. `/var/log` is a
symlink to `/tmp`.

I left an 8.9 MB core dump and a 3.8 MB directory copy in `/tmp` and the daemon was
**OOM-killed** with no message to the process. The only evidence was in `dmesg`:

```
Out of memory: Kill process 1815 (python) score 310
```

Clean up after yourself, and check `df -h /tmp` and `free` when something dies silently.

## The partition layout

For reference, from `/proc/mtd`:

| Partition | Size | Contents |
| --- | --- | --- |
| `mtd0` | 128 KiB | bootstrap |
| `mtd1` | 256 KiB | U-Boot |
| `mtd2`, `mtd3` | 128 KiB each | U-Boot environment |
| `mtd4` | 2 MiB | kernel (`uImage`, magic `27051956`) |
| `mtd5` | 16 MiB | config |
| `mtd6` | 248 MiB | root filesystem, jffs2 |

Hardware is an Atmel `AT91SAM9G20` at 396 MHz, 29 MB usable RAM, kernel 2.6.36.4, uClibc
0.9.33.2, BusyBox 1.21.0, Python 2.7 (UCS2). Ethernet only, no wifi, and the PHY negotiates
10BaseT full-duplex.

## Take a full NAND backup before touching anything

There is **no BERG firmware image anywhere online**. The OTA server died in 2015 and took the
only official copy with it. `kpeeem/berg-bridge-dump` is a filesystem dump, not a flashable
image, and in any case a generic image would not carry your unit's identity: its hostname,
Zigbee NCP configuration and printer pairing state.

Your NAND dump is the only copy that will ever exist. Take it first.

Expect `ECC: N corrected bitflip(s)` messages on a device this age — those are errors ECC
successfully repaired, and the data is intact. `uncorrectable error` is the one that matters;
see `docs/01-nand-bit-rot-libpython.md`.
