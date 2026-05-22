# XP `boot.ini` — archived

This file is archived. The XP boot chain it described — NTLDR loading
`boot.ini` from a FAT32 root, picking between a text-mode bootsector and a
GUI-mode `multi(0)disk(0)rdisk(1)partition(1)\WINDOWS` continuation — was
replaced by the GRUB4DOS + FiraDisk RAM-mapped-ISO design in v0.3.

Current references:
- [`XP_FIRADISK_PIPELINE.md`](XP_FIRADISK_PIPELINE.md) — active design.
- [`HARDWARE_TESTS.md`](HARDWARE_TESTS.md) — GRUB4DOS entry 1 / entry 2
  reboot flow.
- [`BACKLOG.md`](BACKLOG.md) — release blockers and follow-up work.

The historical canonical-recipe comparison, dual-boot footgun analysis,
and WIPE.DAT design notes lived here. They are not part of the shipped
pipeline; usbwin no longer writes `boot.ini` or any NTLDR chain.
