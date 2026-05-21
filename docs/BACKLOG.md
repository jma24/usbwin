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

## Before v0.3

### Release docs for `windows-ntxp`

The user-facing docs should describe the current product shape, not the
recovery process.

Done means:
- README support matrix is current.
- `ARCHITECTURE.md` points at `XP_FIRADISK_PIPELINE.md` for XP design.
- `V0.3_WINDOWS_XP.md` is archived as a short historical pointer.
- Recovery docs are not part of the main README navigation.

### Burn transcript and hardware checklist

The XP release checklist should be repeatable from a terminal transcript.

Done means:
- Document the expected `usbwin` confirmation output for XP SP3 media.
- Document the GRUB4DOS boot choices: entry 1 for text-mode setup, entry 2
  for GUI-mode continuation after reboot.
- Keep the boot-track readback command and expected 64 GB SanDisk MBR entry
  in `HARDWARE_TESTS.md`.

### Pipeline error reporting and verbose mode

The current pipeline has improved top-level context, but hardware/debug
runs still rely on a mix of stdout progress messages and anyhow contexts.
Permission errors, FAT32 mount failures, and staged-file copy failures need
to surface the full error chain cleanly.

Done means:
- Audit `windows_ntxp.rs`, `windows.rs`, and `diskutil.rs` for contexts that
  hide the underlying `io::Error` or command stderr.
- Add a `-v` or `RUST_LOG=usbwin=debug` workflow that shows every file
  operation during a burn.

## Before v1.0

### Windows 2000 install support

Status: 1.0 blocker.

`usbwin-iso` recognizes Windows 2000-style NT5 install media so
`--type=auto` can classify it as NT5-class, but hardware testing on
2026-05-21 showed that this is not enough for a working Windows 2000
install. Do not claim Win2k support in user-facing docs or release notes.

Done means:
- Capture the exact Win2k failure mode from hardware or QEMU.
- Decide whether GRUB4DOS + FiraDisk can support Win2k with compatibility
  changes or needs a separate `windows-2000` mode.
- Add an explicit support matrix row after there is a green path.
- Hardware-verify Windows 2000 install through first desktop boot.

### XP AHCI/SATA/RAID textmode storage support

Status: 1.0 blocker.

XP SP3 does not include broad inbox AHCI support. The current Dell E6410
test path requires BIOS SATA set to ATA mode.

Done means:
- Start narrow with Dell E6410 Intel SATA AHCI.
- Add an explicit experimental flag for a supplied F6 floppy/driver
  directory or a Dell E6410 preset.
- Ensure setup loads both the virtual-CD driver and the internal-disk
  storage driver.
- Hardware-verify XP setup sees and installs to the internal disk with BIOS
  SATA mode set to AHCI.

### XP/2000 unattended support for FiraDisk ISO path

Status: 1.0 blocker.

Done means:
- Generate a derived ISO containing `I386\WINNT.SIF`; never mutate the
  input ISO.
- Support product key, regional/timezone defaults, computer name, admin
  password policy, EULA acceptance, install mode, and driver-signing policy.
- Keep manual partitioning by default. Never set `AutoPartition=1` unless
  the user explicitly opts into destructive full automation.
- Hardware-verify unattended XP install through first desktop boot.

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
