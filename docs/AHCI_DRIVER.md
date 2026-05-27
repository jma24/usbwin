# XP/2003 AHCI textmode driver

`--ahci-driver-dir <path>` slipstreams a vendor F6 driver pack (the same
folder you'd hand to XP's F6 prompt) into the staged XP.ISO's I386
directory and patches `I386\TXTSETUP.SIF` so XP text-mode setup treats
the driver as if it were inbox. PnP auto-binds during PCI enumeration --
no F6, no controller-selection UI, no user interaction.

Status: implemented with QEMU AHCI regression coverage for the historical
`iaStor.sys could not be found` failure. Final Dell Latitude E6410 AHCI
hardware verification is still pending. Reference hardware is QM57 / 5
Series mobile (`8086:3B2F`). Mode is `--type=windows-xp` (alias of
`--type=windows-ntxp`) only.

## What bootsmith does with the folder

1. Reads `txtsetup.oem` from `<path>` as UTF-8.
2. Reads the list of files referenced by per-controller `[Files.scsi.X]`
   sections (`driver=`, `inf=`, `catalog=` lines). For Intel iaStor that's
   `iaStor.sys`, `iaAHCI.inf`, `iaAHCI.cat`, `iaStor.inf`, `iaStor.cat`.
3. Reads the PCI hardware IDs from `[HardwareIds.scsi.X]` sections --
   every `PCI\VEN_xxxx&DEV_yyyy&CC_zzzz` -> service-name pair.
4. Determines the NT service name (third field of `driver=...` lines).
   Typical Intel pack: `iaStor`.
5. **Slipstream:** adds each driver file into the I386 directory of the
   staged XP.ISO. The ISO mutator writes the file data at the end of the
   ISO and rewrites the `/I386` ISO9660 directory with the new records
   sorted into filename order.
6. **Patch:** reads `I386\TXTSETUP.SIF`, inserts new lines into four
   sections, writes the patched file back at a fresh extent. Also
   patches `I386\DOSNET.INF` `[Files]` so the file-copy phase can find
   the slipstreamed binaries.

The four TXTSETUP.SIF sections patched:

| Section | New line |
|---|---|
| `[SCSI.Load]` | `iaStor = iaStor.sys,4` |
| `[SCSI]` | `iaStor = "Intel AHCI Controller"` |
| `[HardwareIdsDatabase]` | `PCI\VEN_8086&DEV_xxxx&CC_yyyy = "iaStor"` (one per HwID) |
| `[SourceDisksFiles]` | `iaStor.sys = 1,,,,,,_x,4,1,,,1,4` (`,,,,,,_x,20,0,0` for .inf/.cat) |

Source disk `1` is the base CD `\I386` source where bootsmith appends the
unpacked driver files. `_x` in field 7 of `[SourceDisksFiles]` is the documented marker for
"file is uncompressed, look it up at the dictionary key verbatim." The
inbox XP CD entries all use `4_` (trailing underscore = LZX-compressed
form `IASTOR.SY_`); slipstreamed binaries are uncompressed so they need
the `_x` marker instead. Empty field 7 is interpreted as the compressed
form and can surface as "The file iaStor.sys could not be found."

The ISO9660 `/I386` directory must remain sorted. XP setup's ISO reader
can miss an otherwise-present slipstreamed file if a new directory record
is simply appended out of order. bootsmith rewrites the directory record set
after each added file so `IAAHCI.INF`, `IASTOR.SYS`, and the other driver
records land in lexical order.

DOSNET.INF additions (last `[Files]` section, lowercase per convention):

```
d1,iastor.sys
d1,iaahci.inf
d1,iaahci.cat
d1,iastor.inf
d1,iastor.cat
```

This is the same recipe nLite and Tim's F6 guide use for adding storage
drivers to a slipstreamed XP install -- the only difference is that
bootsmith runs it from macOS against a freshly-staged copy of the user's
ISO instead of permanently modifying a master XP image.

## Virtual regression test

The `bootsmith-eval` harness can exercise the AHCI file-lookup path without
burning hardware:

```sh
target/release/bootsmith-eval \
  --image /path/to/patched/XP.ISO \
  --boot-media cdrom \
  --flavor windows-xp \
  --target-bus ahci \
  --timeout 600 \
  --interval 10 \
  --mem-mib 768
```

This boots the patched XP ISO as a CD-ROM and attaches the blank target
disk behind QEMU's `ich9-ahci` controller. In AHCI mode the eval only
passes if XP reaches the disk/partition selection screen. It fails on
`iaStor.sys could not be found`, `setup did not find any hard disk
drives`, and BSOD markers.

Current limitation: the Intel R259536 `iaStor` driver gets past the file
lookup point in QEMU, then QEMU's AHCI model BSODs with `STOP 0x0000000A`.
That makes the eval useful for catching ISO/TXTSETUP/DOSNET regressions,
but it is not a substitute for final E6410 hardware verification.

The FiraDisk floppy stays single-purpose: it provides the virtual disk
driver that exposes the GRUB4DOS RAM-mapped CD to text-mode setup. It
does not carry the user's AHCI driver -- that lives in the ISO.

bootsmith does **not** bundle any third-party storage driver. The user
provides the F6 directory.

## Dell Latitude E6410 walkthrough (Intel iaStor 9.5.7.1002, Dell A03)

The reference target. Chipset is Intel QM57 (5 Series mobile); BIOS-AHCI
mode presents `PCI\VEN_8086&DEV_3B2F` ("5 Series 6 Port SATA AHCI
Controller").

1. Download the Dell Update Package from
   <https://www.dell.com/support/home/en-us/drivers/driversdetails?driverid=gw4hh&oscode=ww1&productcode=latitude-e6410>.
   The direct URL is `https://dl.dell.com/SATA/SATA_DRVR_WIN_R259536.EXE`
   (~11 MB). MD5 `7766fa7bb7cb5913b8029172bd1ba58d`.
2. Extract on macOS. This DUP is an InstallShield self-extractor; the
   default Apple tools can't open it, and `unar` doesn't recognize the
   format either. Use p7zip + unshield:

   ```sh
   brew install p7zip unshield
   cd ~/Downloads
   mkdir R259536 && cd R259536
   7z x ../SATA_DRVR_WIN_R259536.EXE
   unshield -d unshield_out x data1.hdr
   ```

   The F6 floppy ends up at
   `R259536/unshield_out/Other_Files/Drivers/x32/` containing
   `TXTSETUP.OEM`, `iaStor.sys`, `iaStor.inf`, `iaStor.cat`, `iaAHCI.inf`,
   `iaAHCI.cat`.
3. Hand that folder to bootsmith:

   ```sh
   sudo bootsmith winxp_sp3.iso /dev/rdisk6 \
        --type=windows-xp \
        --ahci-driver-dir ~/Downloads/R259536/unshield_out/Other_Files/Drivers/x32
   ```
4. Boot the target. Set BIOS SATA Operation to **AHCI** (not ATA, not
   RAID On).
5. At GRUB4DOS pick entry 1 (`XP text-mode setup from RAM ISO`). XP
   prints `Setup is inspecting your computer's hardware configuration...`
   then proceeds directly to the EULA -- **no F6, no controller picker**.
   The slipstreamed driver auto-binds via PCI HwID match.
6. Partition / format / let text-mode finish.
7. Reboot -- usual `windows-xp` flow: pick GRUB4DOS entry 2 to finish
   GUI-mode setup from the internal HDD.

The R259536 pack ships TXTSETUP.OEM entries for 12 AHCI controllers and 5
RAID controllers (ESB2, ICH7/8/9 R/M/MDH variants, ICH10, and Intel 5
Series / 3400 Series). All 17 HwIDs get added to TXTSETUP.SIF's
`[HardwareIdsDatabase]`; PnP binds whichever one is actually on the bus.

## Why slipstream and not the OEM floppy

Earlier drafts of this code tried two other auto-load mechanisms before
landing on slipstream:

1. **Merge user's TXTSETUP.OEM into FiraDisk's floppy + use [Defaults]**:
   XP text-mode setup only auto-loads the ONE driver named in
   `[Defaults]` of an OEM floppy's TXTSETUP.OEM. Other entries require
   F6 + manual controller selection. Empirically validated by the
   WinSetupFromUSB DPMS pattern, which synthesizes two near-identical
   floppies (one per `[Defaults]` value) precisely because single-floppy
   HwID-driven auto-binding doesn't work for non-default drivers.
2. **`winnt.sif` `[MassStorageDrivers]` + `OemPreinstall=Yes`**: this
   activates the OEM driver list, but `OemPreinstall=Yes` makes XP look
   for `I386\$OEM$\Textmode\` on the install source. Our pipeline
   doesn't (and probably shouldn't) inject `$OEM$` directories into the
   staged XP.ISO, so setup bails immediately at `oemdisk.c:1747` with
   error 18 (`ERROR_NO_MORE_FILES`).

Slipstreaming the driver directly into `I386\TXTSETUP.SIF` sidesteps
both problems. The driver is now part of XP's "inbox" view of the
world; standard PCI PnP enumeration binds it without any preinstall
machinery.

## Alternative pack: Intel iaStor 9.6.4.1002 (R274723)

R274723 is a generic Intel Mobile Matrix Storage release with overlapping
but not identical controller coverage. It's a `unar`-extractable DUP and
historically the first pack tested with this code path. R259536 is the
Dell-blessed E6410 build (officially listed on Dell's E6410 driver page)
and is the recommended default. R274723 remains a fine fallback for
non-Dell hardware in the same Intel mobile-chipset family.

## Other vendors

Anything that ships a "Vista-style F6 driver" folder with `TXTSETUP.OEM`
should work. Confirmed-good shape:

```
TXTSETUP.OEM       <- required
<driver>.sys       <- referenced by [Files.scsi.*] driver= lines
<driver>.inf       <- referenced by inf= lines
<driver>.cat       <- referenced by catalog= lines
```

Extra files (READMEs, licenses, x64 variants) are ignored unless the
TXTSETUP.OEM references them.

The slipstream pipeline currently supports one NT service per pack
(every `[Files.scsi.X] driver=` line must name the same third field). A
multi-vendor pack would need separate runs.

## Failure modes

- **`--ahci-driver-dir <path> is not a directory`**: pass the directory
  that contains `TXTSETUP.OEM`, not the .exe or a parent.
- **`<path> has no TXTSETUP.OEM`**: the vendor folder layout is wrong.
  Check for `f6flpy-x86/` or similar inside.
- **`<path> is missing iaStor.sys`** (or similar): the referenced file is
  not in the directory. Vendor packs sometimes split files between
  `f6flpy-x86/` and `f6flpy-x64/`; this code is XP-32-bit only, so the
  x86 folder is the right pick.
- **`declares multiple driver services`**: the pack maps controllers to
  more than one service binary (e.g. `iaStor` plus a separate `iaAHCI`).
  Currently unsupported; pick a single-service pack.
- **`no PCI entries`**: the pack's `[HardwareIds.scsi.*]` only has
  non-PCI ids (e.g. FiraDisk's `detected\firadisk`). Auto-binding via
  PnP needs at least one PCI HwID.
- **`I386 directory has no room for new record`**: the staged ISO's I386
  directory ran out of slack inside its allocated extent for the new
  records. Directory relocation is not yet implemented. Workaround: use
  a smaller driver pack (fewer files) or a different XP ISO.
- **`The file iaStor.sys could not be found`** despite the file being in
  `I386`: check that `/I386` ISO9660 directory records stayed sorted and
  that `TXTSETUP.SIF` uses `_x` for uncompressed slipstreamed files.
  bootsmith has regression coverage for both cases.
- **Disk doesn't appear at the partition screen**: the pack's TXTSETUP.OEM
  doesn't cover the controller's PCI device ID. Boot a Linux live USB on
  the target, run `lspci -nn | grep -iE 'sata|raid'`, and check the
  `[8086:XXXX]` value against the `[HardwareIds.scsi.*]` blocks in the
  pack's TXTSETUP.OEM. If no match, use a different driver pack.
- **0x7B INACCESSIBLE_BOOT_DEVICE at first boot**: BIOS SATA was switched
  from AHCI to ATA between text-mode and GUI-mode, or vice-versa. Keep it
  in AHCI for both.
