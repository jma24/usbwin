# Field findings — 2026-05-18

Empirical lessons from spending ~3h getting a Win 7 install USB to boot on a
Dell E6410 by manually combining UNetbootin + ms-sys on macOS arm64 (no
Rosetta path).  Captures the specific bugs and macOS gotchas that consumed
real time, so usbwin doesn't rediscover them.

---

## 1. ms-sys flag taxonomy is misleading

If forking or building on ms-sys, rename or extensively document:

| Flag | Actually writes | Common misconception |
|---|---|---|
| `-7, --mbr7` | Win 7 **MBR** (sector 0 of whole disk) | "Win 7 boot record" — but only the master boot record, not the partition's. |
| `-2, --fat32nt` | NT 5.x partition boot record that loads **NTLDR** | Name implies "NT-family" but it's specifically XP/2003. Useless for Win 7+. |
| `-e, --fat32pe` | NT 6.x partition boot record that loads **BOOTMGR** | Name implies "PE" (preinstall env) but this is the one Win 7/8/10 install USBs need. |

**A working Win 7 install USB needs BOTH** `--mbr7` AND `--fat32pe`, on the
disk and partition device respectively. Picking `-7` alone (or `--fat32nt`)
gives a USB that boots far enough to print the wrong error and stop.

---

## 2. macOS raw vs. buffered device — silent sub-sector write failure

The bug that cost the most time:

- ms-sys's `write_data()` uses `fseek` + `fwrite` for sub-sector writes (e.g.
  1 byte at offset 0x47, 9 bytes at 0x3f0).
- On **`/dev/rdiskN` (raw)**, sub-sector writes **silently fail**. The
  function returns 0 → ms-sys prints `Failed writing FAT32 PE boot record to
  /dev/rdiskN` with no further diagnostic.
- On **`/dev/diskN` (buffered)**, the kernel handles sub-sector buffering and
  the same writes succeed.
- ms-sys's source has a `#ifdef __FreeBSD__ || __OpenBSD__` path that
  pre-reads aligned sectors and modifies in memory. macOS isn't either, so
  that path is skipped.

**Implications for usbwin**:
- For sub-sector writes, target `/dev/diskN`, not `/dev/rdiskN`.
- For full-sector writes (MBR sector 0 of disk, etc.) `/dev/rdiskN` is 3-5x
  faster — keep using it.
- Or: implement the read-modify-write-whole-sector pattern yourself so you
  can use raw devices uniformly and not depend on this distinction.

---

## 3. Multi-sector boot records — BOOTMGR-loading PBR spans sectors 0 + 1 + 12

The FAT32 PE (Vista/7/8/10) partition boot record is **not** a single
512-byte sector. ms-sys writes data at offsets:
- `0x000` — sector 0, primary boot code + jump
- `0x047` — volume label (sector 0)
- `0x052` — extended boot code (sector 0)
- `0x3f0` — sector 1, secondary code
- `0x1800` — sector 12, tertiary code

A naïve "just write a 512-byte PBR" implementation will miss the sector-1 and
sector-12 parts and produce a bootloader that hangs or prints garbled output.
The sector-12 location is significant: in FAT32, the first 32 sectors of the
partition are reserved (BPB + FSInfo + backup + bootloader extension), and
the boot code can legitimately use any of them.

**Implication**: if shipping embedded boot record blobs, ship all 3 of them
(or 4 if you also handle NTFS). Apply them at the correct offsets.

---

## 4. UNetbootin's specific Win 7 bug (the bug usbwin exists to fix)

UNetbootin's Win 7 USB-creation does:

1. ✅ Format USB as FAT32 with MBR partition table
2. ✅ Mark partition 1 as active (`80` byte at MBR offset 0x1BE)
3. ✅ Copy all Win 7 files: `bootmgr`, `boot/`, `sources/install.wim`, `efi/`
4. ✅ Write a Microsoft-style MBR (strings: `Missing operating system.`,
   `Operating system load error.`, `Multiple active partitions.`)
5. ❌ **DOES NOT replace the FAT32 PBR**. The PBR is still
   `newfs_msdos`'s (OEMid `BSD  4.4`, error string
   `Non-system disk\nPress any key to reboot`).

Result on legacy BIOS:
- BIOS reads USB, runs MBR.
- MBR finds active partition, loads its PBR.
- PBR is the macOS default — prints "Non-system disk" and halts.

So UNetbootin gets ~80% of the way, then leaves the final boot sector in a
macOS-default state. **usbwin must write a Win-compatible PBR with the
correct BPB preserved** to close this gap.

---

## 5. Diagnostic strings to look for

After writing, always re-read the boot records and inspect:

### Sector 0 of partition (PBR) — `xxd -l 16` for OEMid, `strings -a` for boot code messages:

| OEMid (bytes 3-10) | Source | Boots Windows? |
|---|---|---|
| `BSD  4.4` | macOS `newfs_msdos` | ❌ — prints "Non-system disk" |
| `MSWIN4.1` | Microsoft FAT32 PBR | ✅ if accompanied by `BOOTMGR` string |
| `SYSLINUX` / `ISOLINUX` | syslinux family | only via chain-loading |
| `mkdosfs ` | Linux `mkfs.fat` | ❌ |
| `NTFS    ` | Windows NTFS | for NTFS USB images |

Strings to look for:
- `BOOTMGR` + `BOOTMGR is missing` → Vista/7/8/10 PBR ✅
- `NTLDR` + `NTLDR is missing` → XP/2003 PBR (correct only for XP target)
- `Non-system disk` → macOS default, **bug** for Win install USB
- Empty / no recognizable strings → custom or zeroed, likely broken

### Sector 0 of disk (MBR):
- `Missing operating system.`, `Operating system load error.`,
  `Multiple active partitions.` → Microsoft NT-era MBR ✅
- `Invalid partition table` only → DOS-era MBR (also OK for Win 7)
- `ISOLINUX`/`SYSLINUX` → chain-loading bootloader
- Last 2 bytes (`xxd -s 510 -l 2`) must be `55aa`. Else BIOS skips device.

---

## 6. macOS auto-mount races during write sequence

The disk auto-remounts aggressively between operations. Critical:

- After `diskutil eraseDisk`, the disk is auto-mounted.
- After ms-sys writes, the disk often auto-remounts before the next
  command can run.
- "Resource busy" / "device could not be accessed exclusively" errors come
  from this.

Working sequence for chained writes (each line is independent):

```
diskutil unmountDisk /dev/diskN
sudo <write operation>      # runs while unmounted
# disk auto-remounts here
diskutil unmountDisk /dev/diskN
sudo <next write operation> # runs while unmounted
```

usbwin should either:
- Use `DiskArbitration.framework` (`DASessionCreate` +
  `DARegisterDiskAppearedCallback` with a denial callback) to suppress
  auto-mount for the duration of the write, OR
- Bracket every raw write with explicit `diskutil unmountDisk` and accept
  the auto-mount between operations.

The first is cleaner but adds a dependency on the framework; the second is
what we did manually.

---

## 7. Don't use `fdisk` to set the active flag

macOS BSD `fdisk` (`/sbin/fdisk`):
- Looks for `/usr/standalone/i386/boot0` (Intel-era boot template) on every
  invocation. Missing on Apple Silicon. Prints scary warning but works.
- Warns "Device could not be accessed exclusively" if mounted (it'll still
  write).
- The `-y` confirmation prompt is bypassable via `echo`-piping multi-line
  input, but error-prone.

usbwin should write the partition table + active flag directly in code —
it's just 16 bytes per partition entry at MBR offset 0x1BE. Specifically the
first byte of the entry: `0x80` = active, `0x00` = inactive.

---

## 8. Validated end-to-end sequence (Windows mode)

This is what worked, in order:

```
diskutil unmountDisk /dev/diskN
diskutil eraseDisk MS-DOS WIN7 MBR /dev/diskN
diskutil unmountDisk /dev/diskN
sudo fdisk -e /dev/diskN       # f 1, write, y, exit (sets active flag)

diskutil mountDisk /dev/diskN
hdiutil mount path/to/Win7.iso
cp -rp /Volumes/<iso-mountpoint>/* /Volumes/WIN7/
hdiutil unmount /Volumes/<iso-mountpoint>

diskutil unmountDisk /dev/diskN
sudo ms-sys -f --mbr7 /dev/rdiskN       # raw device OK for whole-sector MBR write
sudo ms-sys -f --fat32pe /dev/diskNs1   # BUFFERED device for sub-sector PBR writes

diskutil eject /dev/diskN
```

Total ~5 min on USB 2.0. Boots on real legacy BIOS hardware (verified on
Dell E6410, vintage 2010).

---

## 9. The user-visible failure modes and what they actually mean

If users hit one of these, the diagnostic should auto-detect and suggest:

| User sees | Actual cause |
|---|---|
| `Selected boot device failed` / `No boot device found` | USB not recognized as bootable: missing `55aa` signature, missing MBR partition table, or BIOS USB-boot disabled. |
| `Non-system disk, press any key to reboot` | PBR is `newfs_msdos` default (the UNetbootin bug). |
| `NTLDR is missing` | PBR is NT 5.x (XP-style), target OS is Win 7+. Wrong PBR variant. |
| `BOOTMGR is missing` | PBR is correct but `bootmgr` file wasn't copied to root, or copy is corrupt. |
| Black screen + flashing cursor + sustained USB activity | MBR ran, found active partition, loaded PBR. PBR is reading the FAT32 trying to find a boot file but can't — usually wrong BPB. |
| Win logo appears then "Setup failed to load" | Boot files copied incorrectly (sources/install.wim corrupt or wrong arch). |

---

## 10. Things that didn't matter (negative findings)

These looked suspicious but didn't actually cause problems:

- macOS dialog "The disk you attached was not readable by this computer" —
  click Ignore. It means macOS's FAT32 driver can't parse the (now
  Windows-bootable) PBR, but the bits on disk are correct for the target
  BIOS.
- ms-sys's "Windows 7 master boot record successfully written" message when
  using `-7` on a partition device — it lies, the message is reused across
  all MBR-style writes regardless of target. Verify by re-reading.
- The `description[66]` field in BC's `HiddenSector` containing literal
  text instead of zeros — purely cosmetic, doesn't affect parsing. (This
  one is from a different project but the principle generalizes: don't
  worry about cosmetic deltas that don't change parser behavior.)

---

## Origin

These notes were written by Claude during an unrelated forensics project
(BestCrypt V7 container recovery) where Win 7 was needed in a 2010 Dell
E6410 VM to run a vintage BC installer. The detour to make a bootable USB
on an Apple Silicon Mac without Rosetta took 3h of trial and error.
Findings are empirically validated; the boot chain works end-to-end on the
target hardware.
