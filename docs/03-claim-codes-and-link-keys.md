# Claim codes, link keys, and why a reset printer stops working

If your printer joins the Bridge but never prints, this is almost certainly why.

## Symptoms

In the Bridge's daemon log, forever:

```
Node 0x____ (0x000d6f00________) joined the network.
Trust Center: 0x000d6f00________ STANDARD_SECURITY_UNSECURED_REJOIN: Allow (USE_PRECONFIGURED_KEY)
Link key required for 0x000d6f00________
-> cloud: encryption_key_required
<- cloud: add_device_encryption_key   encryption_key: <some base64>
TC: Network key update requested      return_code 0
...then UNSECURED_REJOIN every 3 seconds, forever
```

Plus:

- Every data send fails with `Send failed, fragment status: 0x00000066`, which is
  `EMBER_DELIVERY_FAILED`.
- Transfers "complete" at the retry timeout: `Completed transfer in 42.6 seconds (35 B/s)`.
  A *real* transfer takes about **0.6 seconds at ~2400 B/s**. That 70x difference is the
  clearest single indicator of a working link.
- The node's short address changes on every join (`0x6882`, `0xf96f`, `0xbbae`...), meaning it
  is joining fresh each time rather than staying on the network.
- `emberBroadcastNextNetworkKey() returned EMBER_INVALID_CALL` appears periodically. It is a
  symptom, not the cause.

## The cause

**The printer was claimed on the server before being factory reset.**

A factory reset generates a **new 40-bit secret** inside the printer, and therefore a new
claim code and a new link key. But the server still holds the key derived from the *old*
claim code, and keeps pushing that.

In Zigbee, a device doing an unsecured join is sent the network key **encrypted with its link
key**. Wrong link key means the printer cannot decrypt the network key, cannot participate,
and rejoins — forever.

## The fix, and why it looks like it does nothing

**Claim the printer again with the new code.** The UI will appear to only rename it. It does
more than that.

In Sirius's `models/user.py`:

```python
hcc = hardware.ClaimCode.query.filter_by(claim_code=claim_code).first()
hardware_xor, _ = claiming.process_claim_code(claim_code)
...
if hcc is None:
    hcc = hardware.ClaimCode(by_id=self.id, hardware_xor=hardware_xor,
                             claim_code=claim_code, name=name)
    db.session.add(hcc)
else:
    pass  # we already have a claim code, don't do anything
```

The lookup is keyed on the **claim code string**, not on the printer. A new code is a
different string, so it inserts a *new* `ClaimCode` row against the same `hardware_xor`, and
the key genuinely is replaced. Only the name change is visible in the UI.

Watch the Bridge log for `add_device_encryption_key` carrying a **different** `encryption_key`
than before. That is your confirmation.

## Verifying a link key yourself

The derivation is deterministic and public, so you never have to guess. `tools/verify_claim_code.py`
in this repo does it, and self-tests against Sirius's own published vector first:

```
6xwh-441j-8115-zyrh  ->  F7D9bmztHV32+WJScGZR0g==
```

The chain is: claim code -> 16 base-32 characters -> 80-bit value -> 40-bit secret -> two
AES-128-ECB rounds over a fixed salt -> 16-byte link key.

Three things worth knowing:

1. **Only the 40-bit secret affects the key.** Two claim codes that differ in the device field
   produce the *identical* key. So a misread character there breaks the CRC but changes
   nothing about the key — which can be very confusing when debugging.
2. **The CRC tells you whether you transcribed it correctly.** Thermal print is easy to
   misread; `0`/`O`, `1`/`l`/`I`, `5`/`S`, `8`/`B`, `x`/`z` are the usual traps. Note the
   alphabet deliberately omits `A`, and maps `I` and `L` to `1`, and `U` to `V`.
3. **The device field must match your printer's address** under Sirius's
   `hardware_xor_from_device_address`, which **reverses the address bytes first**:

   ```python
   def hardware_xor(device_address_hex):
       b = bytearray.fromhex(device_address_hex)
       b.reverse()                      # the crucial step
       c = bytearray(3)
       c[0] = b[0] ^ b[5]
       c[1] = b[1] ^ b[3] ^ b[6]
       c[2] = b[2] ^ b[4] ^ b[7]
       return c[2] << 16 | c[1] << 8 | c[0]
   ```

   **`notjosh/little-printer-claim-code` omits that byte reversal** and does not reproduce
   Sirius's published vector. Do not use it for this check.

Your printer's address is in the Bridge log, as `device_address` in the posted event JSON.

## Known bugs on littleprinter.nordprojects.co

As of October 2026, none of these are your fault:

- **`GET /printer/<id>` can return HTTP 500.** Most likely because re-claiming leaves two
  `ClaimCode` rows for one `hardware_xor` and that page assumes one. The protocol path is
  unaffected and printing still works; you may just be unable to view that page.
- **The claim page still instructs you to point your Bridge at `alpha.littleprinter.com`.**
  That host resolves but answers nothing. Following the instruction will break a working
  setup. Point at `http://littleprinter.nordprojects.co:80/` instead.
- **There is no way to delete or unclaim a printer.** No such route exists anywhere in
  Sirius; `DELETE /printer/<id>` returns 405. Re-claiming with a new code is the only
  mechanism, which fortunately is also the fix above.

## Resetting the printer

For completeness, since BERG's own documentation is gone and the Internet Archive copy of
`bergcloud.com/help/` returns an empty document:

1. **The printer must be plugged in and powered on first.**
2. Remove the metal front and take out the paper roll.
3. Press the reset button through the small hole at the back until it clicks. A paperclip is
   usually too thick — a SIM-tray ejector tool, a straightened staple, a safety pin or a
   multimeter probe tip all fit.
4. **The light on top flashes white.** That is the confirmation the reset took.
5. Reassemble and press the black button next to the light. It prints a claim code.

Blinking lights settling to red mean it is searching and failing, **not** that it needs
resetting. Check your Bridge is actually running first.
