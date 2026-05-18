# Boot records

The single most important technical detail in this codebase. If you remember nothing else, remember this:

## The rule: preserve the BPB when writing the FAT32 PBR

A FAT32 partition's first sector — the "PBR" (Partition Boot Record) or VBR (Volume Boot Record) — is 512 bytes laid out as:

```
offset  size  field
   0      3   jump instruction (x86: EB 58 90)
   3     87   BIOS Parameter Block (BPB) — describes the filesystem geometry
  90    420   boot code (real-mode x86 that loads bootmgr / ntldr from the FS)
 510      2   signature (0x55 0xAA)
```

The **BPB at bytes 3..89 is filesystem geometry** — bytes-per-sector, sectors-per-cluster, root cluster, FAT count, and so on. These values are written by `newfs_msdos` when the partition is formatted and they describe the actual on-disk layout of this specific FAT32 volume.

The **boot code at bytes 90..509 is generic x86** — it reads the BPB to find the FAT, walks the FAT to find `bootmgr` (or `ntldr` for XP), reads it into memory, and jumps to it. The same boot code works on any properly-formatted FAT32 volume because it dynamically reads the BPB at runtime.

### What ms-sys gets wrong

`ms-sys --partition-fat32` writes a **fixed template** of all 512 bytes. The BPB in that template was captured from some long-ago reference Windows install. When you write it to your specific USB, the template BPB no longer matches the partition's actual geometry. The boot code then walks an imaginary FAT and fails to find `bootmgr`.

### What we do instead

```text
read existing_pbr from /dev/rdiskNs1     (512 bytes, just-formatted)
construct merged_pbr:
    bytes   0..2  = our_boot_code[0..2]     (jump)
    bytes   3..89 = existing_pbr[3..89]     (BPB — KEEP)
    bytes  90..509 = our_boot_code[90..509] (boot code)
    bytes 510..511 = 0x55 0xAA              (signature)
write merged_pbr to /dev/rdiskNs1
verify by re-reading and comparing
```

This is the splice. Implemented as `usbwin_boot::pbr::splice_fat32(existing, &OUR_FAT32_BOOT_CODE)`.

## The MBR boot code

The first 440 bytes of `/dev/rdiskN` are the MBR boot code. Same idea but simpler — no BPB to preserve, since the MBR's role is to find the active partition and chain-load its PBR. We write our MBR boot code verbatim. The 64-byte partition table at offset 446 is constructed by us; the 2-byte signature at 510 is fixed.

## How we generate the boot code

`boot-asm/` contains hand-written NASM source for three blobs:

- `mbr.asm` — generic x86 MBR that finds the active primary partition and chain-loads its first sector
- `fat32_pbr.asm` — boot code that reads the BPB, walks the FAT, finds `bootmgr` (Win 7 / 8 / 10 / 11), loads it, jumps
- `ntfs_pbr.asm` — same shape, but walks NTFS structures instead

> A separate XP-era PBR (`fat32_pbr_xp.asm`, loading `NTLDR` instead of `bootmgr`) will land alongside the dedicated `--type=windows-xp` mode in v0.4. The Win 7 PBR is *not* a drop-in replacement for the XP case — see `docs/ARCHITECTURE.md` § MVP target for why XP is its own path.

Build:

```sh
brew install nasm
cd boot-asm
make
```

Output is `boot-asm/build/{mbr,fat32_pbr,ntfs_pbr}.bin`, each 512 bytes. `usbwin-boot/build.rs` invokes the makefile and `include_bytes!`s the results.

## Verifying our boot code is correct

Three layers, in order of feedback-loop tightness:

1. **Byte equality vs ms-sys** (tightest). Set `USBWIN_MSSYS_BLOBS_DIR=/path/to/ms-sys/...` and run `cargo test --features compare-mssys`. The test asserts our NASM output is byte-equal to ms-sys's reference blobs. Equality is sufficient (not necessary) — if we match, we know our code works because ms-sys's has shipped to millions of users. If we diverge, we read both disassemblies side-by-side and figure out who's right.

2. **QEMU boot smoke test**. `cargo test --test qemu_boot` writes a synthetic 64 MiB FAT32 disk image, splices our boot record, and boots it under `qemu-system-i386` with a minimal "kernel" file (named `bootmgr` for the test) that just prints `USBWIN OK\n` to serial. The test scrapes serial output for the string. Pass = our boot record correctly chain-loaded an x86 binary from a FAT32 volume.

3. **Real hardware** ([`HARDWARE_TESTS.md`](HARDWARE_TESTS.md)). The five scenarios from the spec, run manually before each release. Slow feedback loop but the only ground truth.

## The "the bytes match but it still doesn't boot" failure mode

If our blobs are byte-equal to ms-sys's blobs AND the QEMU test passes AND a real machine fails to boot, the problem is **not** the boot record. The fault tree from there:

- BPB is wrong (didn't preserve it; bug in `splice_fat32`)
- Partition isn't marked active (MBR partition table flag)
- `bootmgr` isn't where the boot code expects (root directory? `\bootmgr`?)
- Drive number mismatch (BIOS hands us a drive number; our code uses the wrong one)
- The disk's geometry confuses the BIOS (rare on modern hardware, common on old machines)

`docs/HARDWARE_TESTS.md` enumerates how to bisect each of these.
