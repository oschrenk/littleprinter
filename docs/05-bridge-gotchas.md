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

## Every boot wastes 75 seconds on a dead server

Before starting the daemon, the wrapper runs `updater.pyc`, which tries
`bridge-updates.bergcloud.com`. That host still resolves, so you get a full TCP timeout
rather than a fast failure:

```
netstat: 192.168.x.x:59729  23.23.232.155:443  SYN_SENT
URLError: <urlopen error timed out>
```

Every start pays it, including every restart inside the retry loop. Comment out the
`updater.pyc` invocation and the `.ipk` install loop that follows. Keep the
`/usr/sbin/fw_printenv > /tmp/uboot.env` line: the daemon's `collectEnvironment()` reads it.

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
