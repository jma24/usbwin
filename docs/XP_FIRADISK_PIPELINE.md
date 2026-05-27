# XP FiraDisk pipeline

Current XP design for the `windows-ntxp` path. This started as the
Iteration 1 output from the archived recovery plan: make the boot chain
concrete before writing code or burning hardware.

## Goal

Replace the legacy XP three-tree FAT32 layout with a GRUB4DOS chain that
boots the original XP ISO as a virtual CD and uses FiraDisk to keep that
virtual CD visible after XP setup switches from BIOS INT 13h reads to
protected-mode storage drivers.

This is meant to solve the three current XP regressions as one system:

- The installer reads normal CD-style setup files from the ISO, not
  `\winnt.sif` and mirrored `I386` trees scattered across the USB.
- GRUB4DOS swaps BIOS disk order before setup starts, so the internal
  HDD is `Harddisk0` and the USB is not the default install target.
- GUI-mode setup still sees a CD source because FiraDisk re-exposes the
  RAM-backed ISO after the text-mode handoff.

## Target USB layout

First prototype layout:

```text
/
  grldr
  menu.lst
  XP.ISO
  FIRADISK.IMA
```

Later, if WinVBlock is needed as a fallback:

```text
/
  WINVBLOCK.IMA
```

No `I386`, `$WIN_NT$.~BT`, `$WIN_NT$.~LS`, `NTLDR`, `boot.ini`, `$LDR$`,
`WIPE.DAT`, `ren_fold.cmd`, or `undoren.cmd` should be staged by this
pipeline. Those belong to the legacy path.

## Boot records

The partition remains FAT32 for BIOS compatibility and to keep the USB
readable from macOS and DOS-era tooling.

The production disk uses chenall GRUB4DOS `grldr.mbr` written to the MBR
boot track, plus one active FAT32 LBA partition containing `GRLDR` and
`menu.lst`:

```text
GRUB4DOS MBR/boot track -> active FAT32 partition -> /GRLDR -> /menu.lst
```

Implementation expectation:

- bootsmith embeds the known-working chenall GRUB4DOS 0.4.6a `grldr.mbr`
  bytes and patches only the MBR partition table/signature fields.
- The production bootsmith path should not depend on an unversioned system
  GRUB4DOS install.
- Verify by read-back just like the existing `bootrec` paths.

Production note:

- The first hand prototype used chenall GRUB4DOS 0.4.6a `grldr.mbr`
  written to the MBR/boot track, with the active FAT32 partition entry
  restored into the MBR. That combination successfully reached GRUB4DOS and
  then XP setup in QEMU and on the Dell E6410.
- On the 64 GB SanDisk test stick, the expected MBR entry after burn is:

```text
80 20 21 00 0c fe ff ff 00 08 00 00 b0 02 74 07
```

This decodes as active FAT32 LBA, start LBA 2048, and partition length
125043376 sectors.

Prototype note from 2026-05-20:

- SourceForge GRUB4DOS 0.4.4 `grldr.mbr` did not work in QEMU for this
  image. It scanned the FAT partition but failed with `Cannot find GRLDR`,
  even on a minimal FAT16 control image.
- chenall GRUB4DOS 0.4.6a 2020-08-09 did work with the same hand-written
  MBR/boot-track method.
- `bootlace.com` was not run on macOS; the working prototype wrote
  `grldr.mbr` bytes directly and patched the MBR partition table.

Embedded asset hashes used by the first `windows-ntxp` implementation:

```text
dece3f8d20f84ae0d0fb892b5c3a2d19e7233d0d8885b0027a6f43d77239128d  grldr
f5c6e8e2c1eb7380285fa9cb1c9168e92d5b3b55cde052c043ba81ed17b9acef  grldr.mbr
fc7e8aec711f1655dc02d1193d762177ca7051f690f09f22f18a992e6cd9c5ff  firadisk.ima
```

## GRUB4DOS menu

Prototype `menu.lst`:

```text
timeout 10
default 0

title 1. Windows XP text-mode setup from RAM ISO (FiraDisk)
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /FIRADISK.IMA (fd0)
map --mem /FIRADISK.IMA (fd1)
map --mem /XP.ISO (0xff)
map --hook
root (0xff)
chainloader (0xff)/I386/SETUPLDR.BIN

title 2. Continue Windows XP GUI-mode setup from internal HDD
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /FIRADISK.IMA (fd0)
map --mem /FIRADISK.IMA (fd1)
map --mem /XP.ISO (0xff)
map --hook
root (hd0,0)
chainloader (hd0)+1
```

If direct `SETUPLDR.BIN` chainloading fails to enter XP setup, try this
variant for entry 1:

```text
title 1. Windows XP text-mode setup from RAM ISO (FiraDisk, ISO boot image)
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /FIRADISK.IMA (fd0)
map --mem /FIRADISK.IMA (fd1)
map --mem /XP.ISO (0xff)
map --hook
chainloader (0xff)
```

Expected ordering:

1. Drive swap first: setup should see the internal HDD as BIOS disk
   `0x80` / NT `Harddisk0`.
2. FiraDisk floppy image mapped as both `(fd0)` and `(fd1)` before setup
   starts. Some BIOS/setup combinations probe only one of those slots.
3. XP ISO RAM-mapped as `(0xff)` and hooked before chainload.
4. The same mapping set must be repeated for GUI-mode continuation after
   the text-mode reboot. GRUB4DOS mappings are not persistent across a
   machine reset.

The prototype must capture which chainloader form works in QEMU, because
direct `SETUPLDR.BIN` chainloading is common in prior art but the ISO boot
image path is a useful fallback.

2026-05-20 QEMU note: both direct `chainloader
(0xff)/I386/SETUPLDR.BIN` and ISO boot-image `chainloader (0xff)` reached
the `Windows Setup` ASR prompt, then both failed with identical
`STOP 0x0000000A (0x00000016, 0x00000002, 0x00000000, 0x80812DE1)`.
So the immediate blocker is not the chainload form alone.

## FiraDisk floppy

`FIRADISK.IMA` is a small FAT floppy image containing the FiraDisk textmode
driver package:

```text
/
  TXTSETUP.OEM
  FIRADISK.SYS
  FIRADISK.INF
  FIRADISK.CAT       optional, if shipped with the driver package
```

Prototype expectations:

- Manual F6 driver selection is acceptable for the first QEMU proof.
- A noninteractive production path should avoid an F6 prompt by placing
  the right answer-file directives inside the ISO's `I386\WINNT.SIF`, if
  that proves reliable.
- If FiraDisk fails at protected-mode handoff, repeat with WinVBlock
  before reviving the legacy three-tree design.

The key pass/fail signal is not merely seeing the blue setup screen. It is
getting past the protected-mode setupdd handoff without `0x7B
INACCESSIBLE_BOOT_DEVICE`, then reaching GUI-mode with the source still
available.

## Answer file strategy

Do not mutate the ISO in Iteration 2 unless the driver flow requires it.

Prototype sequence:

1. Boot stock XP ISO + FiraDisk floppy.
2. Use manual F6 driver load if prompted.
3. Confirm the partitioner appears.
4. Confirm text-mode file copy completes.
5. Confirm GUI-mode setup finds the source.

Only after that works should bootsmith add answer-file support.

Production options, in preferred order:

1. Build a derived ISO at burn time with `I386\WINNT.SIF` injected.
   This is explicit and inspectable, but requires ISO9660 authoring.
2. Require the user to provide a pre-customized XP ISO.
   This is easiest to implement but weak UX.
3. Keep `WINNT.SIF` outside the ISO and rely on GRUB4DOS overlay tricks.
   Treat this as speculative until proven in QEMU.

For this pipeline, `WINNT.SIF` belongs in `I386\WINNT.SIF` on the virtual
CD. The legacy root-level `\winnt.sif` convention is not part of the
design.

## Pipeline contract

Input:

- XP ISO path.
- Target USB device.
- Optional FiraDisk driver directory or floppy image.
- Optional WinVBlock fallback driver directory or floppy image.
- Optional XP/2000 answer-file settings.

Output:

- FAT32 USB booting GRUB4DOS.
- `XP.ISO` copied byte-for-byte from input unless answer-file injection is
  enabled. A first `--unattended` implementation rebuilt the ISO with
  `hdiutil makehybrid`, but hardware showed that changed the boot behavior
  before `SETUPLDR.BIN` took over. A root `WINNT.SIF` sidecar was also
  hardware-tested on 2026-05-21 and ignored by XP setup. The current
  implementation writes the generated answer file to two setup-visible
  locations: `A:\WINNT.SIF` inside the staged `FIRADISK.IMA`, and
  `I386/WINNT.SIF` inside the staged copy of `XP.ISO`. The ISO patch appends
  the file and adds an ISO9660 directory record without moving existing ISO
  contents or the El Torito boot image.
  - First hardware signal: after adding `A:\WINNT.SIF` to the FiraDisk
    floppy, XP setup no longer prompted about driver signing. That strongly
    suggests setup is consuming the floppy answer file.
- `FIRADISK.IMA` staged.
- `menu.lst` staged from a deterministic template.

Non-goals:

- No legacy `txtsetup.sif` USB-driver relocation.
- No WaitBT/Wait4UFD injection.
- No NTLDR `boot.ini` menu.
- No destructive wipe bootsector. Drive selection should be handled by
  GRUB4DOS disk mapping and the normal XP partitioner.
- No unattended auto-partitioning by default. Generated answer files set
  `AutoPartition=0` and `Repartition=No`.

## QEMU prototype checklist

Hand-stage a USB image before integrating with bootsmith:

1. Create a FAT32 USB disk image with one active partition.
2. Install GRUB4DOS boot code.
3. Copy `grldr`, `menu.lst`, `XP.ISO`, and `FIRADISK.IMA`.
4. Boot QEMU with two disks: USB image first, blank HDD second.
5. Verify GRUB4DOS loads.
6. Verify XP setup starts from the RAM ISO.
7. Verify the internal HDD is presented as the first setup disk after the
   drive swap.
8. Verify the partitioner UI appears.
9. Verify text-mode file copy completes.
10. Verify GUI-mode setup does not prompt for the CD.

Record for each run:

- Exact GRUB4DOS version.
- Exact FiraDisk or WinVBlock version.
- `menu.lst` used.
- Whether F6/manual driver selection was required.
- Whether `chainloader (0xff)` or direct `SETUPLDR.BIN` worked.
- Whether both `(fd0)` and `(fd1)` mappings were required.
- Whether the ISO had to be loaded with `--mem`. For FiraDisk, assume
  `--mem` is required unless a prototype proves otherwise. Non-RAM ISO
  mapping is lower memory but belongs to the WinVBlock fallback track, not
  the first target.

## Hardware policy

Do not run this on the Dell E6410 until the QEMU prototype reaches
GUI-mode source-found.

First hardware burn should use at least one of:

- HDD physically disconnected.
- Known-sacrificial HDD.
- Full pre/post forensic dump of USB MBR, USB PBR, `menu.lst`, and staged
  files.

The expected post-install invariant is that XP setup does not rewrite the
USB MBR/PBR and does not create `\WINDOWS` on the USB.

## Open questions

- Which chainload form is most robust: ISO boot image or direct
  `I386\SETUPLDR.BIN`?
- Does FiraDisk require manual F6 selection in the minimal prototype?
- What exact `WINNT.SIF` directives are needed to load FiraDisk
  noninteractively from the virtual floppy?
- Does the drive-swap mapping remain visible to XP setup after FiraDisk
  claims the RAM ISO?
- How much RAM should bootsmith require or warn about? `--mem` needs enough
  RAM for the whole ISO plus XP setup overhead; a 512 MiB machine may be
  tight with later SP3 media.
- Should production support WinVBlock as a peer option or only as a debug
  fallback?
- Do we need a Dell-specific workaround such as a modified `NTDETECT.COM`,
  or does the RAM-ISO path avoid the known Dell USB timing problems on the
  E6410?

## Prior art to verify against

- RMPrepUSB Tutorial 30: XP install from ISO on a bootable USB using
  GRUB4DOS, virtual floppy images, drive swap, and a two-entry text/GUI
  menu.
- Easy2Boot XP/DPMS docs: XP ISO boot uses virtual floppies with
  FiraDisk/WinVBlock and keeps the ISO available through protected-mode
  setup.
- MSFN "Install XP from a RAM loaded ISO image" threads: examples of
  `map --mem ... (0xff)` plus direct `I386\SETUPLDR.BIN` chainloading,
  and reports that FiraDisk is the RAM-mapped path while WinVBlock is the
  better candidate for non-RAM mapping.
- Microsoft `txtsetup.oem` documentation: confirms the F6/OEM textmode
  driver package shape that the virtual floppy must present.

## Iteration 2 go/no-go

Go to bootsmith implementation only if QEMU proves:

- XP setup boots from the mapped ISO.
- FiraDisk or WinVBlock prevents `0x7B` at protected-mode handoff.
- The normal partitioner appears.
- GUI-mode setup finds its source without the legacy `I386` trees.

If any of those fail, debug the prototype before touching the bootsmith
pipeline.
