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
| 7 | Win XP SP3 VL                | windows-ntxp | Dell E6410 (legacy BIOS, SATA in ATA mode) | Text-mode + GUI-mode setup both reach completion | ✅ verified 2026-05-21 — production `usbwin --type=windows-ntxp` reached first desktop boot. Text-mode used GRUB4DOS entry 1, GUI-mode continuation used entry 2, and post-test USB sanity check confirmed only the staged GRUB4DOS/FiraDisk payload remained. Post-burn MBR readback verified the GRUB4DOS MBR entry. **Caveat unchanged**: target SATA controller must be in BIOS ATA mode — XP SP3 ships without AHCI drivers; the install can't see an AHCI disk. |

## Bisection guide for "doesn't boot"

When a test fails, run through this checklist before opening an issue:

1. **Does the BIOS see the USB at all?** If no, the partition table is wrong or the disk wasn't initialized. Re-run with `--verbose` and check the printed MBR bytes.
2. **Does the BIOS try to boot but immediately error?** Likely the active flag is missing or the MBR boot code is wrong. Compare MBR bytes against `boot-asm/build/mbr.bin`.
3. **Does the MBR run but say "missing operating system" / "no boot record"?** The PBR is bad. Compare the first sector of the partition (`dd if=/dev/rdisk8s1 bs=512 count=1`) against `boot-asm/build/fat32_pbr.bin` — but remember bytes 3..89 should be the partition's actual BPB, NOT our blob's BPB.
4. **PBR runs but "BOOTMGR is missing"?** The boot code is loading correctly but can't find `bootmgr` in the filesystem. Mount the USB on the Mac and verify `bootmgr` is at the root.
5. **PBR runs, `bootmgr` loads, blue Windows logo, then BSOD/restart?** Not usbwin's fault — that's a Windows install issue (RAM, disk drivers, ISO corruption).

For `windows-ntxp`, the active boot record is the chenall GRUB4DOS
`grldr.mbr` boot track. Verify the MBR partition entry after burn:

```sh
sudo dd if=/dev/rdiskN of=/private/tmp/usbwin-grldr-track.bin bs=512 count=16
xxd -g1 -s 446 -l 66 /private/tmp/usbwin-grldr-track.bin
```

On the 64 GB SanDisk E6410 test stick, the first entry should be:

```text
80 20 21 00 0c fe ff ff 00 08 00 00 b0 02 74 07
```

That decodes as active FAT32 LBA, start LBA 2048, and bounded partition
length 125043376 sectors.

## XP SP3 production transcript

Reference burn:

```sh
sudo usbwin winxp_sp3.iso /dev/rdiskN --type=windows-ntxp
```

Expected confirmation output:

```text
usbwin: formatting FAT32 volume USBWINXP
usbwin: writing GRUB4DOS MBR boot track
usbwin: staged GRLDR, menu.lst, XP.ISO, FIRADISK.IMA
usbwin: winxp_sp3.iso -> /dev/rdiskN (Windows NT/XP FiraDisk mode) OK
```

Boot flow on the Dell E6410:

1. BIOS SATA mode must be ATA.
2. Use F12 and boot the USB stick in legacy BIOS mode.
3. Choose `1. XP text-mode setup from RAM ISO (FiraDisk)`.
4. After text-mode setup reboots, boot the USB again.
5. Choose `2. Continue XP GUI-mode setup from internal HDD`.
6. Continue until XP reaches first desktop boot.

Post-test USB sanity check:

```sh
find /Volumes/USBWINXP -maxdepth 1 -print | sort
```

Expected root payload:

```text
/Volumes/USBWINXP
/Volumes/USBWINXP/FIRADISK.IMA
/Volumes/USBWINXP/GRLDR
/Volumes/USBWINXP/XP.ISO
/Volumes/USBWINXP/menu.lst
```

There should be no `WINDOWS`, `I386`, `$WIN_NT$.~BT`, `$WIN_NT$.~LS`,
`NTLDR`, or `boot.ini` tree on the USB after installation.

## Setup notes

The Dell E6410 is the reference legacy-BIOS machine. F12 boot menu, USB boot enabled in BIOS, no UEFI mode. Any Core-2-era ThinkPad/Latitude/Optiplex works equivalently.

For tests 3 and 4 UEFI mode, any modern desktop with USB boot enabled and Secure Boot disabled.

QEMU substitutes for tests 1, 2, 5, 6 during development (see `tests/qemu_boot.rs`) but should not substitute for a real-hardware test before release tagging.
