#!/usr/bin/env python3
"""Reassemble a file from its jffs2 data nodes in a raw NAND image.

Reports which nodes failed their data CRC and which byte ranges are therefore
missing, which is how you find silent data loss: jffs2 discards a node whose
CRC fails and returns zeros for that range, with no error to userspace. A
checksum of the live file cannot see it.

Nodes are applied in ascending version order, so later writes win.

Usage:
    jffs2_extract.py IMAGE INODE OUTFILE

Find the inode with jffs2_scan.py first.
"""
import struct
import sys
import zlib

MAGIC = 0x1985
NODETYPE_INODE = 0xE002
INODE_HEADER = 68           # sizeof(struct jffs2_raw_inode)

COMPR = {0: 'none', 1: 'zero', 2: 'rtime', 3: 'rubinmips',
         4: 'copy', 5: 'dynrubin', 6: 'zlib', 7: 'lzo'}


def jffs2_crc32(data):
    """jffs2 uses the kernel's crc32 with seed 0 and NO pre/post inversion.

    Getting this wrong makes every node look corrupt, which is a very
    convincing way to waste an hour.
    """
    return zlib.crc32(data, 0xFFFFFFFF) ^ 0xFFFFFFFF


def rtime_decompress(src, destlen):
    positions = [0] * 256
    dest = bytearray()
    sp = 0
    while len(dest) < destlen and sp + 1 < len(src):
        value = src[sp]; sp += 1
        dest.append(value)
        repeat = src[sp]; sp += 1
        backoffs = positions[value]
        positions[value] = len(dest)
        while repeat:
            dest.append(dest[backoffs])
            backoffs += 1
            repeat -= 1
    return bytes(dest[:destlen])


def collect(path, target_ino):
    data = open(path, 'rb').read()
    size = len(data)
    nodes = []
    offset = 0
    while offset < size - 12:
        magic, nodetype, totlen = struct.unpack_from('<HHI', data, offset)
        if magic != MAGIC or totlen < 12 or offset + totlen > size:
            offset += 4
            continue
        if nodetype == NODETYPE_INODE:
            ino, version = struct.unpack_from('<II', data, offset + 12)
            if ino == target_ino:
                isize, = struct.unpack_from('<I', data, offset + 28)
                doff, csize, dsize = struct.unpack_from('<III', data, offset + 44)
                compr = data[offset + 56]
                dcrc, = struct.unpack_from('<I', data, offset + 60)
                payload = data[offset + INODE_HEADER: offset + INODE_HEADER + csize]
                nodes.append(dict(version=version, offset=doff, dsize=dsize,
                                  compr=compr, dcrc=dcrc, isize=isize,
                                  payload=payload, flash_offset=offset))
        offset += (totlen + 3) & ~3
    return nodes


def main(argv):
    if len(argv) < 4:
        print(__doc__.split('Usage:')[1].rstrip())
        return 2

    image, target_ino, outpath = argv[1], int(argv[2]), argv[3]
    nodes = collect(image, target_ino)
    if not nodes:
        print('no data nodes found for inode %d' % target_ino)
        return 1

    good = sum(1 for n in nodes if n['payload'] and jffs2_crc32(n['payload']) == n['dcrc'])
    isize = max(n['isize'] for n in nodes)
    print('data nodes     : %d  (crc ok %d, crc bad %d)' % (len(nodes), good, len(nodes) - good))
    print('compression    : %s' % sorted({COMPR.get(n['compr'], n['compr']) for n in nodes}))
    print('declared size  : %d bytes' % isize)

    buf = bytearray(isize)
    written = bytearray(isize)
    bad_nodes = []

    for node in sorted(nodes, key=lambda n: n['version']):
        if node['dsize'] == 0:
            continue
        crc_ok = bool(node['payload']) and jffs2_crc32(node['payload']) == node['dcrc']
        try:
            if node['compr'] == 0:
                out = node['payload']
            elif node['compr'] == 1:
                out = b'\x00' * node['dsize']
            elif node['compr'] == 6:
                out = zlib.decompress(node['payload'])
            elif node['compr'] == 2:
                out = rtime_decompress(node['payload'], node['dsize'])
            else:
                bad_nodes.append((node, 'unsupported compression %s'
                                  % COMPR.get(node['compr'], node['compr'])))
                continue
        except Exception as exc:
            bad_nodes.append((node, 'decompression failed: %s' % exc))
            continue
        out = out[:node['dsize']]
        buf[node['offset']:node['offset'] + len(out)] = out
        written[node['offset']:node['offset'] + len(out)] = b'\x01' * len(out)
        if not crc_ok:
            bad_nodes.append((node, 'bad data CRC but decompressed; used anyway'))

    if bad_nodes:
        print()
        print('problem nodes:')
        for node, why in bad_nodes:
            print('  v%-5d file range 0x%06x-0x%06x  flash offset 0x%x  %s'
                  % (node['version'], node['offset'],
                     node['offset'] + node['dsize'], node['flash_offset'], why))

    holes = written.count(0)
    print()
    print('unwritten bytes: %d of %d' % (holes, isize))
    if holes:
        start = None
        for i in range(isize + 1):
            filled = i < isize and written[i]
            if not filled and start is None:
                start = i
            elif filled and start is not None:
                if i - start >= 16:
                    print('  hole 0x%06x-0x%06x  (%d bytes)' % (start, i, i - start))
                start = None

    open(outpath, 'wb').write(bytes(buf))
    print()
    print('wrote %s (%d bytes), first 4 bytes %s'
          % (outpath, isize, bytes(buf[:4])))
    print('A hole inside .text is real damage. Scattered short runs of zeros in')
    print('data and string sections are ordinary padding.')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
