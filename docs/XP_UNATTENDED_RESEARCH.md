# XP unattended `WINNT.SIF` injection — research log

Research dossier from 2026-05-21 on how to wire `--unattended` into the
`windows-ntxp` (GRUB4DOS + FiraDisk ramdisk-ISO) pipeline. Two failed
hardware attempts on 2026-05-21, two research-agent reports, and a QEMU
no-op rebuild smoke test conclusively rule out the obvious approaches and
narrow the design space to two viable paths.

## Problem statement

The hardware-verified `windows-ntxp` pipeline:

1. FAT32-formats the USB, installs a Microsoft FAT32 PBR + GRUB4DOS MBR
   (`grldr.mbr`).
2. Stages `GRLDR` + `menu.lst` + `FIRADISK.IMA` + `XP.ISO` (the user's
   original XP SP3 ISO, byte-for-byte) at the FAT32 root.
3. GRUB4DOS does `map --mem /XP.ISO (0xff)` + `map --mem /FIRADISK.IMA
   (fd0)`, then `chainloader (0xff)`. SETUPLDR.BIN runs from the virtual
   CD. FiraDisk's TXTSETUP.OEM lets XP textmode see the ramdisked CD as
   the install source.

`--unattended` needs XP setup (SETUPLDR.BIN → textmode → GUI phase) to
consume a generated `WINNT.SIF` containing product key, computer name,
admin password, timezone, etc.

## Failed attempts (2026-05-21)

### Attempt A — Rebuild XP.ISO with `hdiutil makehybrid` injecting `I386\WINNT.SIF`

Result: GRUB4DOS → SETUPLDR.BIN handoff broke on hardware. `hdiutil
makehybrid` produces hybrid HFS+/ISO9660 — different volume layout that
FiraDisk's `chainloader (0xff)` does not cope with.

### Attempt B — Keep XP.ISO byte-for-byte, stage `WINNT.SIF` at FAT32 root

Result: XP setup completed but ignored the SIF — manual prompts for
product key, computer name, etc. SETUPLDR.BIN only reads
`\I386\WINNT.SIF` from inside the boot volume (the virtual CD). It does
not scan the FAT32 USB root in this boot mode. This attempt did not place
the SIF inside the FiraDisk floppy image, so it did not test the
traditional `A:\WINNT.SIF` path.

### Attempt C — Add `WINNT.SIF` to `FIRADISK.IMA` as `A:\WINNT.SIF`

Result: hardware test stopped showing the previous driver-signing prompt.
That is a strong signal that XP setup is now consuming the generated
answer file from the GRUB4DOS-mapped FiraDisk floppy. This matches the
classic physical-floppy unattended recipe: Setup Manager emits
`unattend.txt`, the user renames it to `winnt.sif`, and CD-booted XP
setup automatically checks the floppy drive.

## QEMU no-op `xorriso` rebuild test

Hypothesis: rebuild the ISO with `xorriso` preserving El Torito, then
inject `\I386\WINNT.SIF`. Smoke-tested on the English XP SP3 VL ISO
(`GRTMPVOL_EN`, El Torito boot image at LBA 345, BIOS no-emul, Ldsiz=4)
under `qemu-system-i386 11.0` with `-cdrom`, `-vga std`, `-m 512`,
direct ISO boot.

Reference: original ISO direct-booted in QEMU reaches the
`IRQL_NOT_LESS_OR_EQUAL` BSOD within 60s — expected without FiraDisk,
confirms XP setup is actually starting.

### `xorriso -indev orig.iso -outdev new.iso -boot_image any replay -commit`

- xorriso patched the El Torito Boot Info Table inside the boot image
  (last 4096 bytes of the boot image differ; first 4096 byte-identical).
- Boot image moved from LBA 345 → LBA 301935 (end of disk).
- `I386\SETUPLDR.BIN` and `I386\TXTSETUP.SIF` SHA-256-identical to
  original.
- **QEMU verdict: garbled text-mode VGA garbage. Never reached BSOD or
  setup screen. Stuck identically across 60s/90s/150s/240s/360s
  snapshots.**

### `xorriso -indev orig.iso -outdev new.iso -boot_image any keep -commit`

- Boot image SHA-256-identical to original (truly byte-preserved).
- xorriso still moved the image to LBA 660 (right after catalog at LBA
  659).
- **QEMU verdict: same garbled VGA garbage at 70s/180s/360s. Stuck
  identically.**

### Root cause

Microsoft XP's CD boot sector is **LBA-coupled**. SETUPLDR.BIN opens
files using absolute LBAs via the El Torito Boot Info Table (offsets
8-63 of the boot image): `bi_PrimaryVolumeDescriptor`,
`bi_BootFileLocation` (in 2K LBA units), length, and a checksum. The
boot image's body also hardcodes additional LBA references.

- `replay`: xorriso repatches the boot-info-table to the new LBA, but
  doesn't (and can't) update the LBA references hardcoded in the image
  body. Diverge → boot fails.
- `keep`: xorriso preserves all bytes but cannot pin the image at LBA
  345. Embedded LBAs are now stale relative to the actual image
  position. Boot fails.

**No `xorriso` / `mkisofs` / `genisoimage` flag pins a file at a
specific source LBA.** `-boot_image any cat_path=` controls only the
catalog. `replay` vs `keep` differ only in boot-info-table repatching;
neither preserves absolute placement of arbitrary files.

## Why WinSetupFromUSB works

WinSetupFromUSB demonstrably installs unattended XP from USB and has for
15+ years. **It does not rebuild the XP ISO.**

For XP/2000/2003 sources, WSFUSB:

1. Expands the source `\I386\` tree onto the USB FAT/NTFS partition
   (NOT inside an ISO).
2. Renames `\I386\` → `\$WIN_NT$.~BT\` (its on-disk convention).
3. Drops `WINNT.SIF` as a plain file at `\I386\WINNT.SIF` on FAT32.
4. Installs USBbootWatcher + WaitBT to keep the install source visible
   across the text-mode → GUI-mode reboot.
5. Uses GRUB4DOS to chainload `\$WIN_NT$.~BT\SETUPLDR.BIN` directly
   from FAT32.

No ISO authoring. No El Torito LBA coupling. The "rebuild a modified
ISO" code path in WSFUSB only fires for NT6 (Vista+), which is a
completely different boot architecture.

## `cat --locate=X --replace=Y` (Easy2Boot trick)

GRUB4DOS can patch the in-RAM copy of the ramdisked ISO after `map
--mem` via `cat --locate=<sentinel> --replace=<bytes>
(0xff)/I386/WINNT.SIF`. RAM copy only — original disk ISO is untouched.

**Length-locked** per chenall/grub4dos issue #122. Replacement bytes
must equal sentinel bytes in byte length. Suitable for small string
edits like flipping `OemPreinstall=Yes`→`No` (Easy2Boot's actual
production use case). Unsuitable for inserting a whole new `WINNT.SIF`
whose size varies by user config — chicken-and-egg with placeholder
creation.

## Viable design paths

### Path 1 — Adopt the WSFUSB pattern (extract, don't ramdisk)

**Mechanic.** On the USB FAT32 partition, expand `\I386\` from the user's
ISO (mount via `hdiutil attach -nomount` + `cp`, or read ISO9660 in pure
Rust via the `iso9660` crate). Rename `\I386\` → `\$WIN_NT$.~BT\`. Drop
`WINNT.SIF` at `\I386\WINNT.SIF`. Replace menu.lst `chainloader
(0xff)/I386/SETUPLDR.BIN` with `chainloader /$WIN_NT$.~BT/SETUPLDR.BIN`
against the USB root. Keep FiraDisk floppy maps. Add USBbootWatcher /
WaitBT for text-mode → GUI-mode handoff so setup finds the install
source after the reboot.

**Pros.** Eliminates the El Torito LBA coupling completely. 15+ years of
WSFUSB field validation behind the recipe. SETUPLDR loads from FAT32 via
GRUB4DOS, not via El Torito. Variable-length SIF works trivially.

**Cons.** Significant architecture change to a hardware-verified
pipeline. Deletes the current ramdisk-ISO path. Requires shipping
USBbootWatcher and WaitBT binaries (where from? license? embedded as
assets?). Requires implementing ISO9660 extraction in pure Rust or
shelling out to `hdiutil`. Big v1 scope creep.

### Path 2 — In-place ISO9660 directory-record mutation

**Mechanic.** Open the ISO file read-write. Append `WINNT.SIF` bytes at
end of ISO, padded to a 2 KiB boundary. Modify only the `\I386`
directory record (in the existing PVD-pointed directory extent) to add
a new entry pointing at the new LBA. Update path table records if
needed. **No existing file moves.** El Torito boot image stays at LBA
345 with all embedded LBAs valid.

**Pros.** Preserves the entire current pipeline. ~150 lines of pure
Rust against the `iso9660` crate (or hand-rolled ISO9660 directory
record encoding). Native arm64 — no Wine, no external binaries. Truly
zero disruption to El Torito.

**Cons.** ISO9660 directory extents are usually allocated tight. `\I386`
holds ~5800 files; need to verify there's slack at the end of its
existing extent. If full, must spill the directory to a new extent —
which means updating path tables to point to the new extent LBA, and at
that point we're closer to a partial rebuild than a pure append.
Need to also update the PVD's volume size and any related fields. No
off-the-shelf tool implements this for XP — bespoke code.

#### Independent check on the XP SP3 VL ISO

The English XP SP3 VL ISO used in hardware testing has:

- PVD volume size: 301638 sectors.
- `\I386` directory extent: LBA 27, declared size 263642 bytes.
- `\I386` directory records: 5888 entries.
- Declared `\I386` byte range: no contiguous end slack; last record ends
  exactly at byte 263642.
- Physical last-sector padding beyond the declared size: 550 zero bytes.
- `WINNT.SIF;1` ISO9660 directory record length: 44 bytes.

So the simplest "write a new entry at the end of the declared directory
extent" variant is false for this ISO. But a narrower append-only variant
is viable: grow the `\I386` directory size to the next 2048-byte boundary,
write the `WINNT.SIF;1` directory record in that existing zero padding,
append the SIF data as a new final sector, and update:

- PVD volume size.
- Root directory's `I386` record size.
- `\I386` self `.` record size.

The path table records do not need to change for this variant because
`I386`'s extent LBA is unchanged and path tables do not carry directory
data sizes.

A `/tmp` prototype of this patch produced an ISO that `xorriso` could
read and extract as `/I386/WINNT.SIF`; `xorriso -report_el_torito` still
reported the BIOS El Torito boot image at LBA 345. This does not replace
the QEMU/hardware proof, but it verifies that Path 2 can be even smaller
than a directory-relocation implementation on the known XP SP3 VL image.

### Path 3 — Microsoft `cdimage.exe` under Wine

`cdimage -lGRTMPVOL_EN -h -n -m -bbootimage.bin -o source out.iso`
produces ISOs in the same layout MS produced (community-verified to
boot). No documented LBA-pin flag — it works because the layout is what
XP's loader was authored against.

**Pros.** Produces verified-bootable ISOs.

**Cons.** Requires Wine on macOS arm64 (Rosetta or crossover-arm). External
non-Rust binary dep. Layout depends on source file mtimes and dedup
mode — fragile. Punts portability and adds a large dependency to ship a
small feature. License terms on bundling `cdimage.exe` are murky.

## Recommendation

**FiraDisk floppy injection plus Path 2** for v1. Put the generated
answer file at `A:\WINNT.SIF` inside the staged `FIRADISK.IMA`, and also
patch `I386\WINNT.SIF` into the staged `XP.ISO` as a fallback. The
floppy path matches the classic XP unattended install flow and produced
the first positive hardware signal: the driver-signing prompt disappeared.
The ISO patch remains useful because it is native, append-only on the
known XP SP3 VL media, and keeps the hardware-verified RAM ISO pipeline.

**Path 1** is the architecturally "correct" long-term answer if usbwin
ever expands beyond unattended-XP into XP driver injection,
multi-source USB, or ISO-less Win2k/2003. Not v1 scope.

**Path 3** is the fallback if Path 2 hits unrecoverable issues with
directory extent overflow on real XP ISOs.

## Verification plan for Path 2

Before any hardware burn:

1. Implement ISO9660 PVD + directory extent + path table parsing.
2. On the original XP SP3 VL ISO, dump the `\I386` directory extent
   layout — total used bytes, total extent size, slack at end.
3. If slack is sufficient: implement the append + directory-record-add
   in a unit test against a copy of the ISO.
4. Mount the modified ISO with `hdiutil attach -nomount` + a manual
   ISO9660 reader; confirm `I386/WINNT.SIF` is listed and points at the
   appended LBA.
5. Direct-boot the modified ISO under QEMU with `-cdrom`; confirm it
   still reaches the same `IRQL_NOT_LESS_OR_EQUAL` BSOD that the
   unmodified original reaches (i.e. boot path is intact; only the SIF
   is new).
6. Stage the modified ISO via the full `windows-ntxp` pipeline; hardware
   test on the Dell E6410.

If step 2 shows insufficient slack on the real ISO, fall back to a
controlled spill: relocate the `\I386` directory extent to the end of
the ISO, update path tables, and accept that some LBAs shift. This
needs a separate QEMU smoke test before hardware.

## Sources

- [WinSetupFromUSB FAQ](https://www.winsetupfromusb.com/faq/)
- [ilko_t on MSFN — how WSFUSB works internally](https://msfn.org/board/topic/120444-how-to-install-windows-from-usb-winsetupfromusb-with-gui/)
- [MSFN: winnt.sif works from floppy, but not I386 folder](https://msfn.org/board/topic/75266-winntsif-works-from-floppy-but-not-i386-folder/)
- [RMPrepUSB Tutorial 101 — cat --locate --replace](https://rmprepusb.com/tutorials/101-patch-any-file-using-grub4dos/)
- [RMPrepUSB Tutorial 30 — XP from ISO via GRUB4DOS](https://rmprepusb.com/tutorials/030-how-to-install-xp-onto-a-hard-disk-from-an-xp-iso-on-a-bootable-usb-drive/)
- [Easy2Boot v1.62 OemPreinstall in-RAM patch](https://rmprepusb.blogspot.com/2014/12/e2b-v162beta1-windows-xp-install-isos.html)
- [chenall/grub4dos issue 122 — cat --replace length lock](https://github.com/chenall/grub4dos/issues/122)
- [xorriso man page — -boot_image keep/replay/patch](https://www.gnu.org/software/xorriso/man_1_xorriso.html)
- [BetaArchive 17313 — cdimage XP ISO command](https://www.betaarchive.com/forum/viewtopic.php?t=17313)
- [El Torito spec — Boot Info Table layout](https://pdos.csail.mit.edu/6.828/2014/readings/boot-cdrom.pdf)
- [Extracting an El Torito Boot Image](https://misc.manty.net/eltorito_extraction.html)
- [WinSetupFromUSB source on SourceForge](https://sourceforge.net/projects/winsetupfromusb/)
- [veewee #434: XP doesn't use winnt.sif from virtual floppy](https://github.com/jedi4ever/veewee/issues/434)
- [Microsoft: How Unattended Installation Works](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-server-2003/cc786944(v=ws.10))
