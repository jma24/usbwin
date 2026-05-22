# Win2k assets — SVBus

Driver payload for the `BootMode::Windows2000` pipeline. See
`../windows_2000.rs` for the pipeline module and
`docs/WIN2K_SVBUS.md` for the design.

## Contents

| file | role |
| --- | --- |
| `svbus.ima` | 1.44 MB FAT12 floppy carrying the F6 textmode driver set. Mapped as `(fd0)` in the GRUB4DOS menu so Windows setup reads it as drive `A:`. |
| `COPYING` | GPL-3.0-or-later licence text, copied verbatim from the upstream archive. |
| `PROVENANCE.md` | Upstream source, archive hashes, driver-signing notes, and the recipe for regenerating `svbus.ima` from upstream. |

`svbus.ima` contains `SVBUSX86.SYS`, `SVBUSX64.SYS`, `SVBUS.INF`,
`SVBUS.CAT`, and `TXTSETUP.OEM`. The `txtsetup.oem` defaults `SCSI =
svbusx86`, so 32-bit Win2k loads the x86 driver automatically; the x64
binary rides along for future x64 NT 5.2/6.x reuse.

## Background

The verified XP path uses GRUB4DOS + FiraDisk + RAM-mapped ISO.
FiraDisk's SCSI miniport is XP/2003+ only and was hardware-confirmed on
2026-05-22 to fail on the NT 5.0 storage stack with
`STOP 0x0000007B 0xF6063848 0xC0000034` (STATUS_OBJECT_NAME_NOT_FOUND
after the duplicate `(fd1)` mapping was removed; STATUS_OBJECT_NAME_COLLISION
before that). SVBus is the grub4dos-org/Kai-Schtrom successor to
WinVBlock and is the canonical NT 5.0-compatible swap-in.

For licence, upstream URL, hashes, and regeneration recipe see
`PROVENANCE.md`.
