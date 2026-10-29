#!/usr/bin/env python3
"""List jffs2 directory entries and inode versions in a raw NAND image.

Useful for finding every historical version of a file, including ones that were
deleted or overwritten. jffs2 is log-structured and does not erase overwritten
data immediately, so on a filesystem that is mostly mounted read-only, old
versions often survive for years.

A dirent with inode 0 is a deletion.

Usage:
    jffs2_scan.py IMAGE [FILENAME]

Example:
    jffs2_scan.py mtd6-root.img libpython2.7.so.1.0
"""
import collections
import struct
import sys

MAGIC = 0x1985
NODETYPE_DIRENT = 0xE001
NODETYPE_INODE = 0xE002
DIRENT_HEADER = 40          # sizeof(struct jffs2_raw_dirent) before the name


def scan(path):
    data = open(path, 'rb').read()
    size = len(data)
    dirents = []
    inode_versions = collections.defaultdict(set)
    offset = 0
    while offset < size - 12:
        magic, nodetype, totlen = struct.unpack_from('<HHI', data, offset)
        if magic != MAGIC or totlen < 12 or offset + totlen > size:
            offset += 4
            continue
        if nodetype == NODETYPE_DIRENT:
            pino, version, ino, _mctime, nsize, _dtype = struct.unpack_from(
                '<IIIIBB', data, offset + 12)
            name = data[offset + DIRENT_HEADER: offset + DIRENT_HEADER + nsize]
            dirents.append((name, ino, version, pino))
        elif nodetype == NODETYPE_INODE:
            ino, version = struct.unpack_from('<II', data, offset + 12)
            inode_versions[ino].add(version)
        offset += (totlen + 3) & ~3
    return dirents, inode_versions


def main(argv):
    if len(argv) < 2:
        print(__doc__.split('Usage:')[1].rstrip())
        return 2

    dirents, inode_versions = scan(argv[1])
    print('dirent nodes            : %d' % len(dirents))
    print('inodes with data nodes  : %d' % len(inode_versions))

    if len(argv) < 3:
        return 0

    target = argv[2].encode()
    hits = [d for d in dirents if d[0] == target]
    print()
    print('dirent entries named %r: %d' % (argv[2], len(hits)))
    for name, ino, version, pino in sorted(hits, key=lambda d: d[2]):
        note = '  <- deletion' if ino == 0 else ''
        print('   ino=%-8d dirent_version=%-5d parent=%-8d data_nodes=%d%s'
              % (ino, version, pino, len(inode_versions.get(ino, ())), note))
    if len(hits) > 1:
        print()
        print('More than one entry means earlier versions of this file may still')
        print('be recoverable. Try jffs2_extract.py on the older inode numbers.')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
