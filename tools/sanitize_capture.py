#!/usr/bin/env python3
"""Turn a raw lp-proxy capture into a fixture that is safe to publish.

Keeps message types, order, relative timing, RSSI, return codes and daemon log
text. Replaces addresses, network identifiers and the U-Boot environment with
fake values, and replaces every print image with a supplied payload, because a
printed image can hold personal content and the print key's first 4 characters.
Refuses to write the output if an original value survives anywhere in it.

Build a payload with lp-encoder, for example:

    lp-encoder set-delivery-and-print 1 client/testdata/barcodehead.png > payload.bin

Usage:
    sanitize_capture.py INPUT.jsonl OUTPUT.jsonl PAYLOAD.bin
"""
import base64
import json
import re
import sys

FAKE_BRIDGE = '000d6f00000000b1'
FAKE_DEVICE = '000d6f00000000d1'
FAKE_MAC = '02:00:00:00:00:01'
FAKE_IP = '192.0.2.27'
FAKE_SERVER = '203.0.113.1:80'
FAKE_PAN = '0x1234'
FAKE_EXT_PAN = '0x4245524700000001'
BASE_TIME = 1700000000.0


def sanitize(lines, payload_b64):
    """Return (output lines, original values that must not survive)."""
    secrets = set()
    start = None
    command_ids = {}

    def shift(t):
        return round(t - start + BASE_TIME, 6)

    def address(value, fake):
        secrets.add(value)
        return fake

    def message(m):
        if 'bridge_address' in m:
            m['bridge_address'] = address(m['bridge_address'], FAKE_BRIDGE)
        if 'device_address' in m:
            m['device_address'] = address(m['device_address'], FAKE_DEVICE)
        if isinstance(m.get('timestamp'), float):
            m['timestamp'] = shift(m['timestamp'])
        if 'command_id' in m:
            cid = m['command_id']
            secrets.add(str(cid))
            m['command_id'] = command_ids.setdefault(cid, 1000 + len(command_ids))
        if m.get('type') == 'DeviceCommand':
            m['binary_payload'] = payload_b64
        for record in m.get('records', []):
            record['created'] = shift(record['created'])

        p = m.get('json_payload')
        if not isinstance(p, dict):
            return m
        if 'device_address' in p:
            p['device_address'] = address(p['device_address'], FAKE_DEVICE)
        if 'uboot_environment' in p:
            raw = p['uboot_environment']
            secrets.add(raw)
            size = len(base64.b64decode(raw + '=' * (-len(raw) % 4)))
            p['uboot_environment'] = base64.b64encode(bytes(size)).decode()
        if 'mac_address' in p:
            p['mac_address'] = address(p['mac_address'], FAKE_MAC)
        if 'local_ip_address' in p:
            p['local_ip_address'] = address(p['local_ip_address'], FAKE_IP)
        net = p.get('network_info')
        if net:
            net['pan_id'] = address(net['pan_id'], FAKE_PAN)
            net['extended_pan_id'] = address(net['extended_pan_id'], FAKE_EXT_PAN)
            net['node_eui64'] = address(net['node_eui64'], '0x' + FAKE_BRIDGE)
        return m

    out = []
    for line in lines:
        d = json.loads(line)
        if start is None:
            start = d['t']
        d['t'] = shift(d['t'])
        if d.get('event') == 'connect':
            secrets.add(d['detail'])
            d['detail'] = FAKE_SERVER
        if d.get('op') == 'text':
            d['original_len'] = d['len']
            d['payload'] = json.dumps(message(json.loads(d['payload'])))
            d['len'] = len(d['payload'].encode())
        out.append(json.dumps(d))
    return out, secrets


def survivors(text, secrets):
    found = set()
    for value in secrets:
        # Short command ids can match inside numbers that mean something else.
        pattern = r'(?<![0-9])' + re.escape(value) + r'(?![0-9])'
        if re.search(pattern, text, re.IGNORECASE):
            found.add(value)
    return found


def self_test():
    env = base64.b64encode(b'serial=SECRET').decode()
    power_on = {'type': 'BridgeEvent', 'bridge_address': 'f00dcafe', 'timestamp': 100.5,
                'json_payload': {'name': 'power_on', 'uboot_environment': env,
                                 'mac_address': '11:22', 'local_ip_address': '10.9.8.7',
                                 'network_info': {'pan_id': '0xBEEF', 'extended_pan_id': '0xCAFE',
                                                  'node_eui64': '0xf00dcafe', 'channel': 24}}}
    command = {'type': 'DeviceCommand', 'bridge_address': 'f00dcafe', 'device_address': 'feedface',
               'command_id': 777777, 'timestamp': '0', 'binary_payload': 'UFJJVkFURQ=='}
    lines = [json.dumps({'t': 100.0, 'event': 'connect', 'detail': '9.9.9.9:80'})]
    for t, m in ((100.5, power_on), (101.0, command)):
        lines.append(json.dumps({'t': t, 'op': 'text', 'len': 1, 'payload': json.dumps(m)}))
    out, secrets = sanitize(lines, 'RkFLRQ==')
    text = '\n'.join(out)
    assert not survivors(text, secrets), survivors(text, secrets)
    assert 'UFJJVkFURQ==' not in text and 'RkFLRQ==' in text
    assert json.loads(out[0])['t'] == BASE_TIME
    assert json.loads(out[2])['t'] == BASE_TIME + 1.0
    frame = json.loads(out[1])
    assert frame['original_len'] == 1 and frame['len'] == len(frame['payload'].encode())
    assert json.loads(frame['payload'])['json_payload']['network_info']['channel'] == 24
    assert survivors('id 777777 here', {'777777'})
    assert not survivors('id 1777777 here', {'777777'})


def main(argv):
    if len(argv) != 4:
        print(__doc__.split('Usage:')[1].rstrip())
        return 2
    self_test()
    payload = base64.b64encode(open(argv[3], 'rb').read()).decode()
    out, secrets = sanitize(open(argv[1]).read().splitlines(), payload)
    text = '\n'.join(out) + '\n'
    left = survivors(text, secrets)
    if left:
        print('refusing to write: %d original value(s) survive' % len(left))
        return 1
    open(argv[2], 'w').write(text)
    print('wrote %s: %d lines, %d values replaced' % (argv[2], len(out), len(secrets)))
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
