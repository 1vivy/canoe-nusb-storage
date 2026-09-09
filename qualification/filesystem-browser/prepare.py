#!/usr/bin/env python3
"""Synthetic fixtures only; never opens a USB device or private capture."""
from pathlib import Path
import re, struct, subprocess
ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'.work/browser-storage/fixtures'
OUT.mkdir(parents=True,exist_ok=True)
def run(*args): return subprocess.check_output(args,stderr=subprocess.DEVNULL,text=True)
for kind in ['fat','ext4','pending']:
    path=OUT/f'{kind}.img'
    with path.open('wb') as stream: stream.truncate(32*1024*1024)
    if kind=='fat': run('mkfs.fat','-F','16',str(path));continue
    run('mkfs.ext4','-q','-F','-b','4096','-I','256','-O','^metadata_csum,^64bit,^orphan_file,uninit_bg','-E','lazy_itable_init=0,lazy_journal_init=0',str(path))
    if kind=='ext4':continue
    # Independent plain-JBD2 encoding, adapted from the maintained dependency's
    # Linux-oracle fixture. Committed seq42 changes inode mode and volume label;
    # uncommitted seq43 must not overwrite that mode. No library writer is used.
    run('debugfs','-w','-R','write /dev/null /oracle',str(path))
    match=re.search(r'located at block (\d+), offset (0x[0-9a-fA-F]+)',run('debugfs','-R','imap /oracle',str(path)))
    inode_block,inode_offset=int(match[1]),int(match[2],16)
    journal=[int(run('debugfs','-R',f'bmap <8> {n}',str(path)).strip()) for n in range(9)]
    image=bytearray(path.read_bytes());block=4096
    def be(data,offset,value):struct.pack_into('>I',data,offset,value)
    def header(kind,sequence):
        data=bytearray(block);be(data,0,0xc03b3998);be(data,4,kind);be(data,8,sequence);return data
    struct.pack_into('<I',image,1120,struct.unpack_from('<I',image,1120)[0]|4)
    jsb=image[journal[0]*block:(journal[0]+1)*block];assert struct.unpack_from('>I',jsb,0x28)[0]==0
    uuid=jsb[0x30:0x40];be(jsb,0x18,42);be(jsb,0x1c,1);be(jsb,0x28,1)
    inode=image[inode_block*block:(inode_block+1)*block];struct.pack_into('<H',inode,inode_offset,0o100600)
    superblock=image[:block];superblock[1144:1160]=b'BROWSERREPLAY'.ljust(16,b'\0')
    descriptor=header(1,42);be(descriptor,12,inode_block);be(descriptor,16,0);descriptor[20:36]=uuid;be(descriptor,36,0);be(descriptor,40,10)
    tail=header(1,43);be(tail,12,inode_block);be(tail,16,8);tail[20:36]=uuid
    uncommitted=bytearray(inode);struct.pack_into('<H',uncommitted,inode_offset,0o100777)
    for n,data in enumerate([descriptor,inode,superblock,header(2,42),tail,uncommitted,bytearray(block)]):image[journal[n+1]*block:(journal[n+1]+1)*block]=data
    image[journal[0]*block:(journal[0]+1)*block]=jsb;path.write_bytes(image)
print(OUT)
