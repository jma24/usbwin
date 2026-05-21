# XP install recovery plan

This file is archived. Active work moved to [`BACKLOG.md`](BACKLOG.md) on
2026-05-21.

The recovery plan was the live operating document for the 2026-05-20 to
2026-05-21 XP remediation pass. Its key outcomes were:

- The legacy three-tree FAT32 XP path was replaced by
  GRUB4DOS + FiraDisk (`windows-ntxp`).
- The hand-staged GRUB4DOS + FiraDisk prototype completed an XP install on
  the Dell E6410 on 2026-05-20.
- The production `windows-ntxp` path reached GUI-mode `Installing Windows`
  with the driver loaded on 2026-05-21.
- Iteration 4 readback verified the GRUB4DOS MBR entry on the 64 GB
  SanDisk: active FAT32 LBA, start LBA 2048, length 125043376 sectors.
- Iteration 5 removed the legacy XP modules and NTLDR/PBR path.
- Iteration 6 added NT5 auto-detection. Windows 2000 classifies as NT5, but
  install support is backlog work.

Current references:
- [`BACKLOG.md`](BACKLOG.md) — active release, cleanup, and compatibility
  work.
- [`XP_FIRADISK_PIPELINE.md`](XP_FIRADISK_PIPELINE.md) — current XP design.
- [`HARDWARE_TESTS.md`](HARDWARE_TESTS.md) — release hardware checklist.

Historical incident log:
- [`XP_REGRESSION_2026_05_20.md`](XP_REGRESSION_2026_05_20.md)
