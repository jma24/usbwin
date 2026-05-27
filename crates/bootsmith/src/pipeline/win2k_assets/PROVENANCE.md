# SVBus provenance

## Upstream

- Project: SVBus — Virtual SCSI Host Adapter
- Author: Kai Schtrom
- Upstream source repository: <https://github.com/grub4dos/svbus> (source only, no release artifacts)
- Upstream binary distribution: <https://sourceforge.net/projects/svbus/>
- Licence: GPL-3.0-or-later (see `COPYING`, copied verbatim from the
  upstream archive's `gpl.txt`).

## Vendored release

| field | value |
| --- | --- |
| Archive | `SVBus_V1.3_20221013.rar` |
| Upstream version | V1.3 |
| Upstream date | 2022-10-13 |
| Vendored on | 2026-05-22 |
| Download URL | <https://downloads.sourceforge.net/project/svbus/SVBus_V1.3_20221013.rar> |
| Archive size | 79,606 bytes |
| Archive MD5 | `eb87f65d6e40d8f837f2384f8a646d01` |
| Archive SHA-256 | `acb55ead3a56fa65e33f8dc2bdc0ce3d3338f875f187df3112012775484f852b` |

The MD5 matches the SourceForge `best_release.json` manifest as of the
vendoring date.

## Files in this directory

| file | source |
| --- | --- |
| `svbus.ima` | 1.44 MB FAT12 floppy image, built locally from the F6 file set inside `SVBus_V1.3_20221013.rar` (see recipe below). |
| `grldr` | GRUB4DOS loader binary, version 0.4.5c (2015-05-18). See "GRUB4DOS version pinning" below. |
| `COPYING` | Verbatim copy of `bin/gpl.txt` from the upstream archive. |

`svbus.ima` contains the five files required by the F6 textmode-driver
mechanism:

| name on floppy | source bytes | size | notes |
| --- | --- | --- | --- |
| `SVBUSX86.SYS` | `bin/svbusx86.sys` (PE-patched) | 19,880 | See "PE subsystem patch for Win2k" below. |
| `SVBUSX64.SYS` | `bin/svbusx64.sys` | 22,952 | Untouched (Win2k is x86-only; x64 ships for future NT 6.x reuse). |
| `SVBUS.INF` | `bin/svbus.inf` | 1,843 | |
| `SVBUS.CAT` | `bin/svbus.cat` | 6,091 | Catalog hashes the unpatched `.sys`; catalog signature does not validate after the PE patch. Win2k does not enforce driver signing, so this is benign there. Do not reuse this floppy on x64 NT 6.x without re-signing. |
| `TXTSETUP.OEM` | `bin/txtsetup.oem` | 672 | |

## PE subsystem patch for Win2k

Upstream `ReadMe.txt` (compile section) notes:

> to support Windows 2000 we have to patch the subsystem version in the PE
> header from 5.02 to 5.00 and correct the PE checksum with LordPE

The pre-built `bin/svbusx86.sys` shipped in `SVBus_V1.3_20221013.rar` has
PE Optional-Header MinorSubsystemVersion = `0x02` (i.e. subsystem version
5.02). Windows 2000 is NT 5.0 and refuses to load drivers whose declared
subsystem version exceeds the OS version, so the unpatched driver
silently fails to bind and text-mode setup BSODs with
`STOP 0x0000007B 0xF6063848 0xC0000034` (STATUS_OBJECT_NAME_NOT_FOUND;
hardware-confirmed 2026-05-22 on the Dell E6410).

The fix is a two-byte edit:

- Set MinorSubsystemVersion at PE-optional-header offset `+50` from
  `0x02` to `0x00`.
- Recompute the PE checksum (Microsoft IMAGEHLP algorithm: 16-bit
  word-sum with fold-and-carry across the file, treating the existing
  checksum field as zero, then add the file length).

For `svbusx86.sys` shipped in V1.3:

| field | before | after |
| --- | --- | --- |
| MajorSubsystemVersion | 5 | 5 |
| MinorSubsystemVersion | 2 | 0 |
| Checksum | `0x0000ce72` | `0x0000ce70` |
| SHA-256 | `a714a1df8528b59dc6d7336b9c4fc4a81368d033f4f8579a17f112f2c4dd4d0e` | `9e2c27e9e4a663ca68a247eade3a9c0d125c71ebc526cefc1c2d4a99f057a5a8` |

The patch is applied by the regeneration recipe below.

## GRUB4DOS version pinning

The Win2k path uses GRUB4DOS `grldr` version **0.4.5c (2015-05-18)**,
NOT the 0.4.6a (2020-08-09) build the XP path uses.

Rationale: SVBus's `DriverEntry` scans the first 640 KB of conventional
memory for the `$INT13SFGRUB4DOS` signature so it can read the GRUB4DOS
drive-map slots. chenall/grub4dos issue
[#154](https://github.com/chenall/grub4dos/issues/154) documents a
low-memory layout regression introduced **2017-02-04** that breaks this
contract; the same regression broke RAM-loaded XP + NTLDR boot
("NTLDR has found only 0K of low memory"). Builds on the broken side of
that line (including the 0.4.6a 2020-08-09 the XP path ships) cause
SVBus to find no signature → enumerate zero drives → text-mode setup
BSODs with `STOP 0x0000007B (… 0xC0000034)`.

Hardware-confirmed on the Dell E6410 on 2026-05-22 with the patched
`svbusx86.sys` + 0.4.6a 2020-08-09 grldr: same BSOD as before the PE
patch, distinct from FiraDisk's collision symptom. Pinning to 0.4.5c is
the documented upstream recipe in the SVBus ReadMe.

The XP path keeps the post-regression 0.4.6a because FiraDisk uses a
different mechanism (XP/2003-era PnP) and does not scan low memory for
the GRUB4DOS signature, so 0.4.6a's regression doesn't impact it.

| field | value |
| --- | --- |
| Upstream URL | <http://dl.grub4dos.chenall.net/grub4dos-0.4.5c-2015-05-18.7z> |
| Release page | <http://grub4dos.chenall.net/downloads/grub4dos-0.4.5c-2015-05-18/> |
| Archive SHA-256 | `ce0ef0f81293470e2405e7b830f3026974b9b5f86cef0b82d2f5b48885da4287` |
| `grldr` size | 283,887 bytes |
| `grldr` SHA-256 | `61bc4fbcbf8e1a4eafca44dcd32209f58afaeaac94acf3e190d8a0381e6aa317` |
| Embedded version string | `GRUB4DOS 0.4.5c 2015-05-18` |
| Vendored on | 2026-05-22 |

We do not vendor `grldr.mbr` from this archive — the XP path's
`ntxp_assets/grldr.mbr` is the mkmsbr-produced boot track and is shared
between the XP and Win2k modes (it only knows how to find and chainload
a file named `GRLDR` on the FAT32 root).

The Windows installer executables (`instx86.exe`, `instx64.exe`) and the
build sources (`SVBus/src/`, `Installer/src/`) are NOT vendored — they
are not needed at install time. In particular, the upstream archive also
contains a `SVBus/src/Verisign.pfx` file which appears to be the
author's code-signing private key bundle and is explicitly NOT vendored.

## Driver signing note

Per upstream `Changes.txt` for V1.3: `svbus.cat`, `svbusx86.sys`, and
`svbusx64.sys` are signed with "a leaked Atheros certificate by Dell".
This is the only practical way to get an unsigned-by-Microsoft kernel
driver to load on Vista x64 / Windows 7 x64+ without disabling Driver
Signature Enforcement. Windows 2000 (this crate's actual target) does
not enforce driver signing at all, so the signature is irrelevant for
the Win2k path; it matters only if SVBus is later reused for x64 NT 6.x
work. Documented here for audit purposes.

## How to regenerate `svbus.ima`

```bash
# Fetch + verify the upstream archive
curl -sL -o /tmp/svbus.rar \
  https://downloads.sourceforge.net/project/svbus/SVBus_V1.3_20221013.rar
test "$(shasum -a 256 /tmp/svbus.rar | awk '{print $1}')" = \
  "acb55ead3a56fa65e33f8dc2bdc0ce3d3338f875f187df3112012775484f852b"

# Extract (requires `unar` from `brew install unar`)
mkdir -p /tmp/svbus
unar -o /tmp/svbus -f /tmp/svbus.rar

# Patch svbusx86.sys PE subsystem version 5.02 -> 5.00 for NT 5.0 / Win2k
cd /tmp/svbus/svbus/bin
python3 <<'PY'
import struct
def pe_checksum(data, chk_off):
    total = 0
    n = len(data)
    for i in range(0, n - (n & 1), 2):
        if chk_off <= i < chk_off + 4:
            continue
        w = data[i] | (data[i+1] << 8)
        total += w
        total = (total & 0xffff) + (total >> 16)
    if n & 1:
        i = n - 1
        if not (chk_off <= i < chk_off + 4):
            total += data[i]
            total = (total & 0xffff) + (total >> 16)
    total = (total & 0xffff) + (total >> 16)
    total &= 0xffff
    return (total + n) & 0xffffffff

with open('svbusx86.sys', 'rb') as f:
    data = bytearray(f.read())
e_lfanew = struct.unpack_from('<I', data, 0x3C)[0]
opt = e_lfanew + 24
struct.pack_into('<H', data, opt + 50, 0)        # MinorSubsystemVersion -> 0
struct.pack_into('<I', data, opt + 64, 0)        # zero checksum
new_chk = pe_checksum(bytes(data), opt + 64)
struct.pack_into('<I', data, opt + 64, new_chk)
with open('svbusx86.sys', 'wb') as f:
    f.write(data)
PY

# Build 1.44 MB FAT12 floppy (requires `mtools` from `brew install mtools`)
dd if=/dev/zero of=/tmp/svbus.ima bs=1024 count=1440
mformat -i /tmp/svbus.ima -f 1440 ::
mcopy -i /tmp/svbus.ima svbusx86.sys ::SVBUSX86.SYS
mcopy -i /tmp/svbus.ima svbusx64.sys ::SVBUSX64.SYS
mcopy -i /tmp/svbus.ima svbus.inf    ::SVBUS.INF
mcopy -i /tmp/svbus.ima svbus.cat    ::SVBUS.CAT
mcopy -i /tmp/svbus.ima txtsetup.oem ::TXTSETUP.OEM

# Drop into the repo
cp /tmp/svbus.ima crates/bootsmith/src/pipeline/win2k_assets/svbus.ima
```

The resulting `svbus.ima` is **not bit-reproducible** across machines:
FAT12 records file mtimes and a random volume serial number at format
time, so the SHA-256 of the floppy depends on when/where you built it.
Reproducible content inside the floppy is what matters — verify by
`mdir -i svbus.ima ::` and matching the file table above.

The SHA-256 of the floppy as vendored 2026-05-22 was
`6185976dfdc61eb0d3475e1c815cfaba9aafb6f243e7db59cf23af3bd9c09242`,
recorded here for forensic reference only.
