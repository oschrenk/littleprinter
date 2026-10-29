# Serial console and getting root

Corrections to the published procedure, all measured on real hardware.

## The device you open is the Bridge, not the printer

BERG never released firmware for the printer. It is a Zigbee end device with no console and
nothing to flash. The serial header is in the **Berg Cloud Bridge**, the white box.

Confusingly, FTDI calls its own chip a "USB to UART bridge" too. Below, **adapter** always
means the USB-serial board and **Bridge** always means the BERG box. Every signal name
carries the board it belongs to, because both have a TX.

## The pinout is published in reverse

The three-hole row is silkscreened `SERIAL`. **`GND` is the square pad**, at the end nearest
the reset button.

Holes, left to right with the square pad on the right:

| Hole | Signal | Connect to |
| --- | --- | --- |
| left | `Bridge RX` | `adapter TX` |
| middle | `Bridge TX` | `adapter RX` |
| right (square) | `Bridge GND` | `adapter GND` |

**pipt's post lists the order from the square pad**, i.e. the other way round. Wiring it that
way produces total silence, which looks exactly like a dead board.

Verify it yourself in 30 seconds, with all leads off and the Bridge powered: probe each hole
against the square pad. **The Bridge's TX idles at 3.3 V**, because that is what a driving
UART output does. The RX hole sits at 0 V. Whichever hole reads ~3.3 V is `Bridge TX` and
goes to your adapter's RX.

Connect `GND` first, so both boards share a reference before any line is driven. Do not
connect `VCC`. 115200 8N1.

### Splitting the fault if you get silence

An adapter loopback costs nothing and halves the problem. Take both signal leads off the
Bridge, join adapter TX to adapter RX (a paperclip pushed into both female sockets works),
and type. If characters echo, the adapter, cable, terminal and baud rate are all fine and the
fault is at the Bridge end.

## U-Boot: stop the prompt timing out

Tap the spacebar continuously *before* plugging power in, and keep tapping for about five
seconds. The window at `Hit any key to stop autoboot:` is roughly one second.

**Then make `setenv bootretry -1` your very first command.** Without it, U-Boot resets after
about a second of console inactivity, so any pause to read a note loses the prompt. This one
step makes the rest of the work possible.

## `single` does nothing. Use `init=/bin/sh`

The Bridge runs BusyBox init, which does not read `single` from the kernel command line.
Appending it reaches the `login:` prompt and then the stock reboot loop.

```
setenv bootretry -1
setenv bootargs "${bootargs} init=/bin/sh"
printenv bootargs
boot
```

Append with the **variable reference**, not a retyped string. Stock `bootargs` is about 200
characters, and a paste that long drops characters at 115200 with no flow control, leaving
U-Boot on its `>` continuation prompt waiting for a closing quote (`Ctrl-C` aborts).

No `saveenv`, so the next boot is normal again.

**The root device is `/dev/mtdblock6`, not the `mtdblock5` in pipt's post.** Never paste the
guide's full `bootargs` over your own. Independently confirmed by `wiki.bytewerk.org`, which
also documents the `init=/bin/sh` approach.

You land at `/ #` with `/bin/sh: can't access tty; job control turned off`, which is expected.

## Setting a root password

Nothing mounted `/proc`, because nothing ran init:

```sh
dmesg -n 1                      # stop ECC noise flooding the console
mount -t proc proc /proc
mount -o remount,rw /
passwd
sync
grep ^root /etc/shadow          # confirm the hash before pulling power
```

**The hash is 13 characters of DES `crypt`, not `$1$` MD5, so only the first 8 characters of
your password are significant.** Choose exactly 8 ASCII letters and digits; a longer
passphrase is silently truncated, and SSH will later accept any string whose first 8
characters match.

Then **pull power**. Do not run `reboot`: with `init=/bin/sh` there is no init to shut
anything down, and `sync` has already flushed the write.

## Paste reliability

115200 with no flow control means a paste can outrun the UART and drop characters. In `screen`,
`Ctrl-A` then `f` cycles flow control and helps. The real answer is to stop using the serial
console as soon as SSH works: type one short `ifconfig` line, then do everything over the
network.

## SSH, once you have root

`dropbear` 2012.55 is installed but disabled; see `docs/05-bridge-gotchas.md`. Modern OpenSSH
needs legacy algorithms:

```sh
ssh -oKexAlgorithms=+diffie-hellman-group1-sha1 -oHostKeyAlgorithms=+ssh-rsa root@BRIDGE_IP
```

`+ssh-rsa` matters as much as the key exchange, because OpenSSH 8.8 disabled SHA-1 RSA host
keys. Guides that list only `+ssh-dss` are incomplete.

**Replace the host keys.** The factory ones are dated 2014 and are therefore identical on
every Bridge ever shipped, so anyone with another unit or a firmware image can impersonate
yours. Dropbear 2012.55 speaks RSA and DSS only, so drop DSS entirely since modern OpenSSH
refuses `ssh-dss`:

```sh
rm /etc/dropbear/dropbear_dss_host_key /etc/dropbear/dropbear_rsa_host_key
dropbearkey -t rsa -s 2048 -f /etc/dropbear/dropbear_rsa_host_key
sync
```

Then install a key so you stop typing that password. Dropbear reads
`/root/.ssh/authorized_keys`, and it must be an **RSA** key — this build predates ed25519:

```sh
ssh-keygen -t rsa -b 2048 -f ~/.ssh/bridge_rsa -N ""
cat ~/.ssh/bridge_rsa.pub | ssh -oKexAlgorithms=+diffie-hellman-group1-sha1 \
  -oHostKeyAlgorithms=+ssh-rsa root@BRIDGE_IP \
  'mount -o remount,rw / && mkdir -p /root/.ssh && cat >> /root/.ssh/authorized_keys \
   && chmod 600 /root/.ssh/authorized_keys && sync && echo KEY_INSTALLED'
```
