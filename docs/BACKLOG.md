# Backlog

Active release and cleanup work. Historical debugging notes live in
`RECOVERY_PLAN.md`, `TECH_DEBT.md`, and `XP_REGRESSION_2026_05_20.md`; do
not treat those files as the current work queue.

Last updated: 2026-05-21.

## v1.0 scope

usbwin 1.0 is a focused Windows installer USB tool, not a generic boot
loader. The target matrix is Windows 2000, Windows XP, and Windows 7, with
unattended install support and NT5-era AHCI/textmode storage support.
Linux/isolinux, generic UEFI-only media, Windows 8+, and broad rescue-disk
coverage are useful follow-up work, but they are not 1.0 blockers. Generic
ISO writing is already covered by tools like `dd`; the 1.0 value is making
old Windows installers work reliably from macOS.

## Release blockers

None.

## Completed for v0.3

### XP production path: first desktop boot

Status: done 2026-05-21.

The hand-staged GRUB4DOS + FiraDisk prototype completed an XP install on
2026-05-20. The production `windows-ntxp` path reached first desktop boot
on the Dell E6410 on 2026-05-21, and post-burn readback verified the
GRUB4DOS MBR entry on the 64 GB SanDisk.

Completed:
- XP SP3 reached first desktop boot from the production `usbwin` USB.
- Post-test USB sanity check confirmed the USB still contained only the
  staged GRUB4DOS/FiraDisk files and no `\WINDOWS` install tree.
- `HARDWARE_TESTS.md` row 7 was updated from pending to green.

### Release docs for `windows-ntxp`

Status: done 2026-05-21.

- README support matrix reflects the FiraDisk path.
- `ARCHITECTURE.md` points at `XP_FIRADISK_PIPELINE.md` for XP design.
- `V0.3_WINDOWS_XP.md` is a short archival pointer.
- `XP_BOOT_INI.md` is a short archival pointer (FiraDisk path replaced the
  boot.ini chain).
- README links to `ARCHITECTURE.md`, `XP_FIRADISK_PIPELINE.md`, and
  `BACKLOG.md` only — recovery/regression docs are not in the main nav.

### Burn transcript and hardware checklist

Status: done 2026-05-21.

- `HARDWARE_TESTS.md` has the expected `usbwin` confirmation output for XP
  SP3 media.
- The GRUB4DOS boot choices (entry 1 text-mode, entry 2 GUI-mode
  continuation) and reboot flow are documented in the boot flow section.
- The boot-track readback command and expected 64 GB SanDisk MBR entry are
  documented alongside row 7.

## Before v1.0

### Post-FiraDisk-migration cleanup

Status: code/docs pass done 2026-05-21. Only the FAT32 cluster-size
hardware question (see Cleanup below) remains.

The FiraDisk migration replaced the old NTLDR / boot.ini / I386-staging
pipeline with GRUB4DOS + RAM-mapped ISO. The leftover NTLDR-era code and
docs have been removed:

- Empty `crates/usbwin/src/pipeline/xp_assets/` directory deleted. ✅
- `crates/usbwin/src/pipeline/fat32.rs` deleted (was the FAT32 walker for
  the never-shipped `build_xp_setup_chain_bootsect` LDR$ loader). ✅
- `boot_records.rs::build_mbr_xp` and `tests/golden/mbr_xp_64gb.bin`
  deleted. XP mode writes the GRUB4DOS `grldr.mbr` boot track; no MBR_XP
  or MBR_WIN7 is involved. ✅
- `boot_records.rs` doc comments updated to reflect that the Win 7 path
  writes MBR_WIN7 and the XP path writes GRLDR_MBR. ✅
- `docs/XP_BOOT_INI.md` reduced to a short archival pointer. ✅

### Windows 2000 install support

Status: 1.0 blocker. Text-mode install works on the Dell E6410
(verified 2026-05-22). First boot of the installed Win2k requires a
manual `boot.ini` repair (see "Win2k boot.ini auto-repair (phase 3)"
below). GUI-mode setup and first-desktop boot are expected to work
after the repair but are not yet hardware-validated through to
completion.

Full root-cause story and working install procedure live in
`docs/WIN2K_SVBUS.md`. Quick summary of what's actually shipping:

- `BootMode::Windows2000` + `--type=windows-2000` (alias `win2k`)
  plumbed end-to-end. ISO classifier splits NT5 media on WIN51/WIN52
  markers (present -> XP/2003 path, absent -> Win2k path).
- SVBus V1.3 vendored from SourceForge with `svbusx86.sys` PE
  subsystem version patched 5.02 -> 5.00 for NT 5.0 compatibility.
  See `crates/usbwin/src/pipeline/win2k_assets/PROVENANCE.md`.
- GRUB4DOS 0.4.5c (2015-05-18) vendored for the Win2k path
  specifically (XP keeps 0.4.6a).
- menu.lst entry 1: no `hd0/hd1` swap, El Torito chainload
  (`chainloader (0xff)`). F6 + manual "SVBus Virtual SCSI Host
  Adapter x86" selection is required at the early text-mode setup
  screen.
- menu.lst entry 2: swap + `chainloader (hd0,0)/ntldr`. Works only
  after the boot.ini repair below.

Remaining work to call this "done" for v1.0:
- Hardware-verify GUI-mode setup completes through to first desktop
  boot AFTER the manual boot.ini repair (Option A or B in
  `docs/WIN2K_SVBUS.md`). Strongly expected to work; iteration this
  week stopped at the boot.ini issue itself.
- Ship the phase 3 auto-repair (next item) OR document the manual
  repair step as part of the supported procedure. Either is
  acceptable for 1.0.
- Add an explicit support-matrix row in the README once the
  end-to-end path is green.

### Win2k boot.ini auto-repair (phase 3)

Status: v1.0 polish item. Probably ships before 1.0 to remove the
manual step; if not, the manual procedure in `docs/WIN2K_SVBUS.md`
becomes the documented 1.0 path.

**The conflict** (now hardware-validated 2026-05-22):

- SVBus's text-mode slot enumeration breaks if GRUB4DOS does the
  `hd0/hd1` swap during install (BSOD 0x7B/0xC0000034). Entry 1
  must run with no swap.
- Win2k's text-mode setup writes boot.ini's `rdisk(N)` based on the
  BIOS-visible disk ordering at install time. With no swap, USB is
  0x80 and the internal HDD is 0x81 -> setup writes `rdisk(1)`.
- BUT: NTLDR + NT PBR + ARC-path resolution all hard-code that the
  system disk is BIOS drive 0x80. To boot the installed Win2k via
  GRUB4DOS chainload on the second BIOS HDD, you need the swap (so
  internal HDD becomes 0x80). With the swap, `rdisk(1)` resolves
  to the USB, not the HDD -> ntoskrnl missing. boot.ini needs
  `rdisk(0)`.
- USB hot-removal during install was tested and corrupts the
  install (BIOS caches the USB; Win2k still sees it; setup writes
  the boot loader to the USB instead of the HDD).
- GRUB4DOS 0.4.5c's in-place NTFS write rejects boot.ini with
  "Error 16 Fatal cannot write resident/small file! Enlarge it to
  2Kb and try again" because boot.ini is small enough to be MFT-
  resident. No way to enlarge from outside the FS.

**The fix**: a third menu entry that boots a small environment,
mounts the NTFS partition, rewrites `rdisk(1)` -> `rdisk(0)` in
`C:\boot.ini`, and reboots. Implementation options ranked by
maintainability:

1. **Tiny Linux initrd** (Tinycore/Alpine, ~20 MB asset cost):
   boots, mounts via ntfs-3g, runs `sed`, reboots. Most robust; the
   kernel handles all NTFS edge cases. **Recommended.**
2. **Automated Recovery Console flow**: chain to setupldr with a
   pre-staged response file that runs `set AllowAllPaths = TRUE` +
   `copy con c:\boot.ini` + the new content. No extra binaries
   shipped, but Recovery Console isn't really designed for
   automation and the procedure hasn't been hardware-verified end
   to end (`set AllowAllPaths = TRUE` was suggested by upstream
   research but never tested in our iteration).
3. **FreeDOS + NTFS write tool** (~2 MB): smaller than Linux but
   the FOSS NTFS-write story is unmaintained.
4. **GRUB4DOS raw-sector patch of the MFT entry**: smallest
   footprint, no extra asset. Compute LBA of boot.ini's MFT record,
   patch the resident data bytes. Fragile -- one NTFS layout
   variation away from corrupting the volume.

Done means:
- Pick one implementation (recommendation: option 1).
- Ship as a third GRUB4DOS menu entry; document in the NEXT STEPS
  block as the recommended post-install step.
- Hardware-verify the full install path: install -> reboot ->
  phase 3 (boot.ini fixed) -> reboot -> native boot -> GUI-mode ->
  first desktop.
- Remove the manual boot.ini repair procedure from the NEXT STEPS
  block once phase 3 is hardware-proven across multiple machines.

### XP AHCI/SATA/RAID textmode storage support

Status: 1.0 blocker.

XP SP3 does not include broad inbox AHCI support. The current Dell E6410
test path requires BIOS SATA set to ATA mode. Web research (2026-05-21)
established the driver direction and minimum textmode driver set; not
yet hardware-validated.

Research findings (need hardware validation):
- Without an Intel `iaStor.sys` / `iaAHCI.inf` F6 driver, AHCI-mode setup
  will not see the HDD → expected `STOP 0x7B INACCESSIBLE_BOOT_DEVICE`
  pointed at the internal disk rather than the install medium.
- Canonical driver for the E6410 (ICH9M-E, VEN_8086 DEV_3B29):
  Intel Matrix Storage Manager F6 floppy, `iaStor` family. Dell still
  hosts a working build under the Latitude E6410 support pages
  (driver ID `rr1rk`, IMSM 9.6.4.1002 A00). Intel pulled their own
  listings years ago.
- USB controller drivers are NOT required for the usbwin RAM-mapped
  chain (FiraDisk/SVBus take over INT 13h from RAM after `map --mem`).
- USB 3.0 forward-compat is a docs problem, not a code problem — users
  on newer machines need to plug into a USB 2.0 port and/or enable
  legacy USB support in BIOS. Document this rather than coding around
  it.

Done means:
- Start narrow with Dell E6410 Intel SATA AHCI.
- Add an explicit experimental flag (`--ahci-driver-dir <path>`) that
  consumes a vendor-shaped F6 folder (`txtsetup.oem` + `iaStor.sys` +
  `iaAHCI.inf` + `iaStor.cat`). Do not bundle the Intel driver — user
  supplies it.
- Stage the AHCI driver alongside FiraDisk so text-mode setup loads
  both the ramdisk filter and the AHCI miniport. Mechanism (single
  merged `txtsetup.oem` vs two virtual floppies) TBD during
  implementation — both are documented in the literature.
- Hardware-verify XP setup sees and installs to the internal disk with
  BIOS SATA mode set to AHCI on the E6410.
- README + HARDWARE_TESTS note the USB 2.0 port + legacy-USB-support
  requirements for non-reference hardware.

### XP unattended support for FiraDisk ISO path

Status: done 2026-05-21. Implementation landed in commit 22f00ec and was
hardware-verified end-to-end on the Dell E6410.

- `--unattended` injects `I386\WINNT.SIF` into the staged `XP.ISO` and
  `A:\WINNT.SIF` into the staged `FIRADISK.IMA`; the input ISO is never
  mutated. ✅
- Supports product key, computer name, admin password policy, timezone,
  EULA acceptance, install mode, and driver-signing policy. ✅
- Keeps manual partitioning by default (`AutoPartition=0`). ✅
- Hardware-verified: unattended XP install reaches first desktop boot on
  the E6410 with a real product key. ✅

Win2k unattended falls under the Windows 2000 install item below — same
SIF mechanics but verified against a Win2k ISO.

### Windows 7 release hardening

Status: 1.0 blocker.

Done means:
- Re-run the Win 7 SP1 hardware test after the XP-path cleanup.
- Confirm `--type=auto` and explicit `--type=windows` both produce the
  expected Win 7 boot path.
- Keep `--boot-record=ms-sys` as an audit fallback, but document the
  in-process mkmsbr backend as the default release path.

### Release packaging

Status: 1.0 blocker.

Done means:
- Build signed/notarized macOS release binaries.
- Remove the local sibling `../mkmsbr` requirement from release builds by
  using a published crate, vendored dependency, or pinned git dependency.
  Implemented with the published `mkmsbr` crate; needs fresh-machine
  install verification.
- Update README install instructions for users who are not building from a
  local multi-repo checkout.

### Pipeline error reporting and verbose mode

Status: 1.0 blocker.

The current pipeline has improved top-level context, but hardware/debug
runs still rely on a mix of stdout progress messages and anyhow contexts.
Permission errors, FAT32 mount failures, and staged-file copy failures need
to surface the full error chain cleanly.

Done means:
- Audit `windows_ntxp.rs`, `windows.rs`, and `diskutil.rs` for contexts that
  hide the underlying `io::Error` or command stderr.
- Add a `-v` or `RUST_LOG=usbwin=debug` workflow that shows every file
  operation during a burn.

## Compatibility backlog

### WinVBlock and low-RAM fallback

Status: backlog.

FiraDisk + RAM ISO remains the default. Some machines may need WinVBlock or
a reduced-ISO path when RAM is tight.

Done means:
- Add controlled debug switches for FiraDisk-only, WinVBlock-only, and
  combined driver floppy images.
- Add a RAM requirement warning before burning a full RAM-mapped XP ISO.
- Keep the default path simple unless hardware evidence says otherwise.

### Generic Linux/isolinux and UEFI-only modes

Status: post-1.0.

The codebase still has explicit mode names for isolinux and UEFI-only
media, but v1 is Windows-focused. Do not spend 1.0 time turning usbwin into
a generic boot loader.

Done means:
- Decide whether to keep the current mode names as future placeholders or
  hide them from help output until implementation starts.
- Implement only after the Windows 2000/XP/7 scope is shipped.

## Cleanup

### FAT32 cluster-size assumption

`pipeline::diskutil::newfs_msdos_fat32` forces 4 KiB clusters with `-c 8`.
This was added during XP debugging because of a suspected setupldr
compatibility issue with 32 KiB clusters on large FAT32 partitions. The
actual failure mode was never isolated.

Done means:
- Test the XP path with the default `newfs_msdos` cluster sizing on a large
  USB stick.
- If it fails, document the precise failure mode.
- If it works, remove the forced `-c 8`.

### Historical remediation docs

Status: mostly collapsed.

Done.

- `RECOVERY_PLAN.md` is archival only.
- `TECH_DEBT.md` points to this backlog.
- `V0.3_WINDOWS_XP.md` is a historical pointer.
- `XP_REGRESSION_2026_05_20.md` remains as a historical incident log.
- No active docs instruct contributors to read remediation files first.
