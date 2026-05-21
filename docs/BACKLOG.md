# Backlog

Active release and cleanup work. Historical debugging notes live in
`RECOVERY_PLAN.md`, `TECH_DEBT.md`, and `XP_REGRESSION_2026_05_20.md`; do
not treat those files as the current work queue.

Last updated: 2026-05-21.

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

## Compatibility backlog

### Windows 2000 support

Status: backlog.

`usbwin-iso` recognizes Windows 2000-style NT5 install media so
`--type=auto` can classify it as NT5-class, but hardware testing on
2026-05-21 showed that this is not enough for a working Windows 2000
install. Do not claim Win2k support in user-facing docs or release notes.

Done means:
- Capture the exact Win2k failure mode from hardware or QEMU.
- Decide whether GRUB4DOS + FiraDisk can support Win2k with compatibility
  changes or needs a separate `windows-2000` mode.
- Add an explicit support matrix row only after there is a green path.

### XP AHCI/SATA/RAID textmode storage support

Status: backlog.

XP SP3 does not include broad inbox AHCI support. The current Dell E6410
test path requires BIOS SATA set to ATA mode.

Done means:
- Start narrow with Dell E6410 Intel SATA AHCI.
- Add an explicit experimental flag for a supplied F6 floppy/driver
  directory or a Dell E6410 preset.
- Ensure setup loads both the virtual-CD driver and the internal-disk
  storage driver.

### WinVBlock and low-RAM fallback

Status: backlog.

FiraDisk + RAM ISO remains the default. Some machines may need WinVBlock or
a reduced-ISO path when RAM is tight.

Done means:
- Add controlled debug switches for FiraDisk-only, WinVBlock-only, and
  combined driver floppy images.
- Add a RAM requirement warning before burning a full RAM-mapped XP ISO.
- Keep the default path simple unless hardware evidence says otherwise.

### XP unattended support for FiraDisk ISO path

Status: backlog after XP production path is green.

Done means:
- Generate a derived ISO containing `I386\WINNT.SIF`; never mutate the
  input ISO.
- Support product key, regional/timezone defaults, computer name, admin
  password policy, EULA acceptance, install mode, and driver-signing policy.
- Keep manual partitioning by default. Never set `AutoPartition=1` unless
  the user explicitly opts into destructive full automation.

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
