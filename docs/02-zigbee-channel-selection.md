# The Bridge picks a Zigbee channel the printer never scans

A genuine bug in BERG's own daemon. If you hit it, the printer can never find the Bridge, no
matter what else is correct.

## Symptoms

- The printer's lights blink, then settle to **red**. It is searching and failing.
- The Bridge's debug page at `:81/magic_debug` shows an **empty packet list** and an empty
  device radio summary. It is receiving zero Zigbee frames from anything.
- Everything else looks healthy: daemon running, `EMBER_NETWORK_UP`, `Setting pjoin(255)`,
  cloud connection established, all LEDs as expected.

Zero received packets with a working radio is the tell. A joining Zigbee device sends beacon
requests on *its* scan channels; the Bridge only listens on *its* channel. If they do not
overlap, both sides are working perfectly and neither hears the other.

## The bug

`api.py` creates the network on the correct channels:

```python
self.logger.info('Attempting to create PAN on channels %s' % self.bridge_configuration['channel_list'])
self.scanForUnusedPanId(self.bridge_configuration['channel_list'])
```

where `channel_list` is `[11, 14, 15, 19, 20, 24, 25]` — the set the printer scans.

Then `weminuche_bridge.run()` immediately performs an energy scan and **moves the network to
the quietest channel out of all 16**, never consulting `channel_list`:

```python
self.device.energyScanRequest(0)
...
(total, failed, energies,) = app.device.collectAndClearChannelScanResults()
lowest_energy = 0
selected_channel = 0
for energy in energies:
    (channel, dbm,) = energy
    if dbm < lowest_energy:
        selected_channel = channel
        lowest_energy = dbm
app.device.channelChangeRequest(selected_channel)
```

So the Bridge builds its network where the printer looks, then walks away. On my unit it
chose **17** and **23** on different runs. Neither is a channel the printer scans.

Whether you hit this is pure luck: it depends on which channel happens to be quietest in your
RF environment. In a noisy place — an office, a block of flats — you are more likely to land
outside the valid set.

## Confirming which channel you are on

The daemon logs it, but **only if logging is enabled**, which it is not by default:

```sh
# in /usr/bin/oneshot_bergcloud_bridge.sh, add --logfile to PY_DAEMON_OPTS
PY_DAEMON_OPTS="--daemon --cloudlog --webdebug --logfile"
```

Then after a restart:

```sh
grep -i "switching to channel" /var/log/bergcloud_bridge.log
```

Beware: `--logfile-level` takes a **numeric** level. The code does `int(options.logfile_level)`,
so `--logfile-level INFO` raises `ValueError` and kills the daemon silently before any
logging exists. Use `10` for debug, `30` for warnings, or omit it.

Note `/var/log` is a symlink to `/tmp`, which is a tmpfs, so logs live in RAM and vanish on
reboot. The handler is also a `RotatingFileHandler(..., 262144, 0)` — a 256 KB cap with
**zero** backups, so it truncates itself rather than rotating.

## The fix

You cannot easily patch `weminuche_bridge.pyc`, but you can shadow the module that supplies
the scan results. Filtering them means the daemon's own "pick the lowest energy" loop can
only choose a valid channel.

Save the original first, then create `/usr/local/bergcloud-bridge/network_manager.py`:

```sh
cd /usr/local/bergcloud-bridge
mount -o remount,rw /
cp -a network_manager.pyc network_manager_orig.pyc
```

```python
# network_manager.py
import imp as _imp
import os as _os

_ORIG_NAME = '_network_manager_orig'
_dir = _os.path.dirname(_os.path.abspath(__file__))
_orig = _imp.load_compiled(_ORIG_NAME, _os.path.join(_dir, 'network_manager_orig.pyc'))

for _k in vars(_orig):
    if _k not in ('__name__', '__file__', '__builtins__', '__doc__', '__package__'):
        globals()[_k] = getattr(_orig, _k)

ALLOWED_CHANNELS = (11, 14, 15, 19, 20, 24, 25)

_real_collect = NetworkManager.collectAndClearChannelScanResults


def _collect_only_allowed_channels(self):
    total, failed, energies = _real_collect(self)
    filtered = [e for e in energies if e[0] in ALLOWED_CHANNELS]
    if not filtered:
        # never hand back an empty list; the caller would keep channel 0
        self.logger.info('Channel filter matched nothing, using the full scan')
        return (total, failed, energies)
    self.logger.info('Channel filter: %d of %d channels allowed %s'
                     % (len(filtered), len(energies), [e[0] for e in filtered]))
    return (total, failed, filtered)


NetworkManager.collectAndClearChannelScanResults = _collect_only_allowed_channels
```

Then `rm network_manager.pyc`, `sync`, and restart. Undo is `rm network_manager.py` plus
`cp network_manager_orig.pyc network_manager.pyc`.

**Important:** back up the `.pyc` *before* creating the `.py`. Python will overwrite the
`.pyc` from your new source, destroying the original bytecode.

## What success looks like

```
Channel filter: 7 of 16 channels allowed [11, 14, 15, 19, 20, 24, 25]
Switching to channel 19 with lowest energy -71
Setting pjoin(255)
Node 0x____ (0x000d6f00________) joined the network.
```

`000d6f00` is the Ember/Silicon Labs OUI, so a node with that prefix joining is your printer.
