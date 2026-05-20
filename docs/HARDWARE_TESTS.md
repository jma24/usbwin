# Hardware test plan

Manual tests, run before each tagged release. Each test produces a USB stick from a known ISO and boots it on real legacy hardware.

## Scenarios

| # | ISO                          | Mode        | Target hardware            | Pass criteria                                  | Status |
|---|------------------------------|-------------|----------------------------|------------------------------------------------|--------|
| 1 | Win 7 SP1 32-bit             | windows     | Dell E6410 (legacy BIOS)   | Installer reaches "Install now" screen         | ✅ verified 2026-05-19 (bootrec backend) |
| 1b | Win 7 SP1 32-bit            | windows + `--boot-record=ms-sys` | Dell E6410 (legacy BIOS) | Installer reaches "Install now" screen | ✅ verified 2026-05-19 (ms-sys backend) |
| 2 | Win 10 22H2                  | windows     | Same machine, BIOS mode    | Installer reaches "Install now" screen         | TODO   |
| 3 | Win 10 22H2                  | auto        | UEFI desktop               | Installer reaches "Install now" screen         | TODO   |
| 4 | Ubuntu 22.04 (hybrid)        | auto        | Both BIOS and UEFI machine | GRUB boot menu appears, kernel loads           | TODO   |
| 5 | FreeBSD 14                   | auto        | Legacy BIOS                | FreeBSD loader prompt appears                  | TODO   |
| 6 | Hiren's BootCD PE            | windows     | Legacy BIOS                | Hiren's menu appears                           | TODO   |
| 7 | Win XP SP3 VL                | windows-xp + `--boot-record=bootrec` | Dell E6410 (legacy BIOS, SATA in ATA mode) | Text-mode + GUI-mode setup both reach completion | ✅ verified 2026-05-19 — full boot chain: `mkmsbr MBR_WIN7 → mkmsbr NTLDR PBR → \NTLDR → boot.ini → BOOTSECT.DAT (mkmsbr raw-LBA loader) → \$LDR$ → setupldr → text-mode setup → reboot → GUI-mode setup (reads source from `C:\$WIN_NT$.~LS\I386\` staged on target HDD; `ren_fold.cmd`/`undoren.cmd` keep USB names sane across the text→GUI transition)`. **Caveat**: target SATA controller must be in BIOS ATA mode — XP SP3 ships without AHCI drivers; the install can't see an AHCI disk. |
| 7b | Win XP SP3 VL                | windows-xp + `--boot-record=ms-sys` | Dell E6410 (legacy BIOS) | Text-mode setup reaches partitioner | ❌ 2026-05-19 — flashing-cursor hang immediately after PBR loads. ms-sys `--fat32nt` PBR isn't loading `\NTLDR` correctly on this BIOS; bootrec's PBR works in the equivalent slot. ms-sys-side PBR investigation deferred (bootrec is the active path). |
| 7c | Win XP SP3 VL                | windows-xp + boot.ini wipe entry (`\WIPE.DAT`)   | Dell E6410 (legacy BIOS) | NTLDR menu shows three entries; entry 3 displays `target=0x81 size=NNNNNN MiB / USB=0x80 safe / Y=wipe?`; Y wipes LBA 0..2047 of HDD and reboots; subsequent text-mode setup installs onto the virgin disk cleanly | ✅ verified 2026-05-20 — found one bug on first attempt: 16-bit `div bx` overflowed on a >64 GB HDD and hung the CPU at `target=0x81 size=_` with a flashing cursor. Fixed by switching to 32-bit `div ebx` for both the MiB conversion and `print_dec`'s digit-extraction loop. Re-tested after fix: full wipe → reboot → fresh install completes. |

## Bisection guide for "doesn't boot"

When a test fails, run through this checklist before opening an issue:

1. **Does the BIOS see the USB at all?** If no, the partition table is wrong or the disk wasn't initialized. Re-run with `--verbose` and check the printed MBR bytes.
2. **Does the BIOS try to boot but immediately error?** Likely the active flag is missing or the MBR boot code is wrong. Compare MBR bytes against `boot-asm/build/mbr.bin`.
3. **Does the MBR run but say "missing operating system" / "no boot record"?** The PBR is bad. Compare the first sector of the partition (`dd if=/dev/rdisk8s1 bs=512 count=1`) against `boot-asm/build/fat32_pbr.bin` — but remember bytes 3..89 should be the partition's actual BPB, NOT our blob's BPB.
4. **PBR runs but "BOOTMGR is missing"?** The boot code is loading correctly but can't find `bootmgr` in the filesystem. Mount the USB on the Mac and verify `bootmgr` is at the root.
5. **PBR runs, `bootmgr` loads, blue Windows logo, then BSOD/restart?** Not usbwin's fault — that's a Windows install issue (RAM, disk drivers, ISO corruption).

## Setup notes

The Dell E6410 is the reference legacy-BIOS machine. F12 boot menu, USB boot enabled in BIOS, no UEFI mode. Any Core-2-era ThinkPad/Latitude/Optiplex works equivalently.

For tests 3 and 4 UEFI mode, any modern desktop with USB boot enabled and Secure Boot disabled.

QEMU substitutes for tests 1, 2, 5, 6 during development (see `tests/qemu_boot.rs`) but should not substitute for a real-hardware test before release tagging.
