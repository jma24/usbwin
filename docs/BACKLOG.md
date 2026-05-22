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

Status: 1.0 blocker. Scoping in `WIN2K_SVBUS.md`.

`usbwin-iso` recognizes Windows 2000-style NT5 install media so
`--type=auto` can classify it as NT5-class, but hardware testing on
2026-05-21 showed Win2k reaches text-mode setup and then BSODs (stop code
not captured). Do not claim Win2k support in user-facing docs or release
notes.

Web research on 2026-05-21 established the root cause and the
implementation direction:

- **FiraDisk does not support Windows 2000** (XP/2003+ only; confirmed via
  reboot.pro thread 8168 and the FiraDisk changelog on thread 8804). The
  most likely BSOD is `STOP 0x7B INACCESSIBLE_BOOT_DEVICE` because
  FiraDisk's SCSI miniport never enumerates under NT 5.0, so setupldr
  loses the boot volume at the real-mode → protected-mode handoff.
- **SVBus** (the grub4dos-org successor to WinVBlock, github.com/grub4dos/
  svbus) is the canonical Win2k-compatible swap-in. Its ReadMe documents
  a verbatim Win2k SP4 install recipe and the GRUB4DOS chain shape is
  nearly identical to the current XP `FIRADISK.IMA` path.
- USB controller drivers (`usbehci.sys`/`usbohci.sys`) **probably don't
  matter** for the RAM-mapped-ISO architecture (follow-up research:
  GRUB4DOS `map --mem` puts the ISO in RAM and INT 13h is serviced from
  the buffer, so setup never reads from USB post-handoff). The MSFN 147119
  BSOD is specific to WinSetupFromUSB's sector-mapped path. **Not
  validated on hardware yet.** Conclusions above (FiraDisk-vs-Win2k root
  cause, SVBus direction, USB-driver irrelevance) all need confirmation
  via the diagnostic capture step before code work begins.

Implementation direction: add a separate `windows-2000` mode that swaps
`FIRADISK.IMA` for `svbus.ima` and ships a Win2k `txtsetup.oem` template.
Keep the verified XP path on FiraDisk (zero regression risk).

Done means:
- Capture the actual stop code on the next E6410 run (photograph the
  `*** STOP: 0x...` line; add `/sos` to setupldr to print each driver as
  it loads). This validates the FiraDisk hypothesis before any code work.
- Vendor SVBus into the repo with provenance pinned to a specific
  grub4dos/svbus release.
- Build the `windows-2000` mode: GRUB4DOS chain reuses the XP shape,
  staged floppy carries SVBus instead of FiraDisk, F6 `txtsetup.oem`
  references "SVBus Virtual SCSI Host Adapter x86".
- Only reintroduce the `txtsetup.sif` USB-driver patch if SVBus alone
  doesn't get to first desktop boot.
- Hardware-verify Windows 2000 install through first desktop boot on the
  E6410.
- Add an explicit support matrix row after the green path lands.

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
