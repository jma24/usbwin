# Windows 2000 install via GRUB4DOS + SVBus

State of the Win2k path as of 2026-05-22. Hardware-verified on the Dell
Latitude E6410 (BIOS SATA in ATA-compat mode, single internal SATA HDD).

## Current status

**Text-mode install: works.** GRUB4DOS + RAM-mapped ISO + SVBus floppy
+ F6 + manual "SVBus Virtual SCSI Host Adapter x86" selection runs all
the way through partition, format, and file copy.

**First boot of installed Win2k: requires manual `boot.ini` repair.**
Win2k's text-mode setup writes `rdisk(1)` in `boot.ini` because at
install time the USB stick is BIOS drive 0x80 and the internal HDD is
0x81. Both native boot and the GRUB4DOS entry-2 chainload need
`rdisk(0)` (the post-swap drive numbering NT loaders hard-code).
Repair is currently a manual Recovery Console / Linux-live-USB step.
Automating it is the v1.0 polish item tracked in
`docs/BACKLOG.md` ("Win2k boot.ini auto-repair (phase 3)").

**GUI-mode setup: untested past the boot.ini repair**, because the
manual repair step has blocked us from reaching it cleanly in
iteration. Expected to work since GUI-mode setup runs from the HDD's
`\$WIN_NT$.~LS` directory with no further install-media dependency.

## How we got here (compressed)

The Win2k path turned out to be a stack of independent gotchas, each
masking the next. In rough chronological order of discovery on
2026-05-22:

1. **FiraDisk (the XP driver) crashes on NT 5.0** with
   `STOP 0x7B 0xC0000035 / 0xC0000034` (depending on mapping
   duplication). FiraDisk's SCSI miniport was never validated against
   the NT 5.0 storage stack — its INF lacks version decoration so it
   loads, but it collides on device-name registration.
2. **SVBus is the canonical NT 5.0-capable replacement** for FiraDisk
   in the grub4dos community. Upstream:
   <https://github.com/grub4dos/svbus>, redistributed as
   `SVBus_V1.3_20221013.rar` on SourceForge.
3. **SVBus's `svbusx86.sys` ships with PE Optional-Header
   MinorSubsystemVersion = 5.02 (XP/2003).** NT 5.0 refuses to load
   drivers whose declared subsystem version exceeds the OS version. We
   patch the byte at PE-optional-header `+50` from `0x02` to `0x00`
   and recompute the PE checksum at vendor time. See
   `crates/bootsmith/src/pipeline/win2k_assets/PROVENANCE.md`.
4. **GRUB4DOS 0.4.6a (the version the XP path uses) doesn't matter
   here in practice** — early research worker flagged
   chenall/grub4dos#154 as a low-memory regression that breaks
   SVBus's signature scan, but the actual symptom on hardware was
   identical with both 0.4.6a and 0.4.5c. The version swap was
   ultimately harmless but isn't load-bearing. We ship 0.4.5c for
   the Win2k path anyway because that's the version SVBus's own
   ReadMe specifies — keeps bootsmith's deviation from upstream minimal.
5. **The `map (hd0) (hd1) / map (hd1) (hd0)` swap breaks SVBus during
   text-mode setup.** With the swap in place, SVBus's GRUB4DOS-slot
   enumeration ends up with zero usable drives and text-mode setup
   BSODs `0x7B/0xC0000034`. Entry 1 (text-mode) must NOT swap.
6. **El Torito chainload (`chainloader (0xff)`) is required for entry
   1**, not direct `(0xff)/I386/SETUPLDR.BIN`. Direct setupldr load
   produces a BSOD at a different stage — text-mode setup expects to
   come up via the CD's El Torito entry so the boot device registers
   as a CD-ROM correctly.
7. **NTLDR + NT PBR + boot.ini ARC-path resolution hard-code that the
   system disk is BIOS drive 0x80**, regardless of where it was
   installed from. To boot the installed Win2k via GRUB4DOS chainload
   on the *second* BIOS HDD, you have to swap so internal HDD becomes
   0x80. Without the swap, NTLDR's INT13 reads target the USB and
   fail in various ways (MS MBR "Error loading operating system", NT
   PBR "Disk read error", or NTLDR "Invalid BOOT.INI" depending on
   how deep into the chain the read fails).
8. **boot.ini's `rdisk(N)` value is computed at install time** based
   on the BIOS-visible drive count and ordering — `rdisk(0)` is the
   first BIOS HDD, `rdisk(1)` the second. Because entry 1 doesn't
   swap (#5), the install always writes `rdisk(1)` even though the
   only correct value for both native boot and the post-install
   swap-chainload (#7) is `rdisk(0)`. **This is the conflict at the
   heart of the Win2k path**: SVBus's text-mode requirement (no
   swap) is incompatible with Win2k setup's boot.ini correctness
   (which needs the internal HDD to look like 0x80 during install).
9. **USB hot-removal at the "Press any key" prompt does NOT work
   around #8 on the E6410.** Tested 2026-05-22: BIOS caches the USB
   in its POST disk table, Win2k still sees two drives during
   enumeration, and the removal corrupts the install path (Win2k
   writes the system loader to the USB instead of the internal HDD,
   destroying the GRUB4DOS chain on the USB).
10. **GRUB4DOS 0.4.5c can write to NTFS files**, but only to
    non-resident files. `boot.ini` is ~200 bytes and gets stored
    resident in the MFT, so an in-place GRUB4DOS write fails with
    `Error 16 Fatal cannot write resident/small file! Enlarge it to
    2Kb and try again` — and there's no way to enlarge a resident
    file from outside the filesystem. The automated fix needs a
    different mechanism (see phase 3 in BACKLOG).

## Working install procedure (hardware-tested 2026-05-22)

1. `bootsmith <win2k.iso> /dev/rdiskN --type=windows-2000`
2. Boot USB on target. GRUB4DOS menu → entry 1.
3. "Press any key to boot from CD" → press a key. **Leave USB in.**
4. At first text-mode screen ("Setup is inspecting your computer's
   hardware…"): press **F6**. Then **S**. Select **SVBus Virtual SCSI
   Host Adapter x86**. Enter, Enter.
5. Continue setup. Delete any existing partition; create a primary
   partition (2–4 GB is fine for a test install); format NTFS.
6. Setup copies files and reboots.

**At this point the install is on disk but neither native nor
GRUB4DOS-chain boot works** because boot.ini has `rdisk(1)`. Manual
repair, two options:

### Option A — Win2k Recovery Console (preferred when it works)

Boot USB → entry 1 → press any key → F6 → S → SVBus → at Welcome to
Setup screen press **R** → **C** → pick install → empty admin
password. At `C:\WINNT>`:

```
set AllowAllPaths = TRUE
attrib -h c:\boot.ini
attrib -r c:\boot.ini
attrib -s c:\boot.ini
copy con c:\boot.ini
[boot loader]
timeout=1
default=multi(0)disk(0)rdisk(0)partition(1)\WINNT
[operating systems]
multi(0)disk(0)rdisk(0)partition(1)\WINNT="Windows 2000" /fastdetect
```

Then **Ctrl+Z**, **Enter**. `exit` to reboot.

This procedure has NOT been hardware-verified end-to-end. `set
AllowAllPaths = TRUE` was suggested by the research worker but never
tested in our iteration; if Recovery Console still rejects writes
after this, fall through to option B.

### Option B — Linux live USB (definitive fallback)

Boot any Linux live USB (Ubuntu, Tinycore, etc.). Mount the internal
HDD's NTFS partition. Open `boot.ini` in any editor. Change both
occurrences of `rdisk(1)` to `rdisk(0)`. Save. Reboot.

### After the repair

Two ways to boot:

- **Native (USB removed)**: BIOS boots the internal HDD directly.
  NTLDR reads the repaired boot.ini, finds ntoskrnl at rdisk(0) (now
  the only disk), GUI-mode setup auto-resumes on first boot.
- **USB-in-place, GRUB4DOS entry 2**: the swap chain in entry 2 makes
  internal HDD look like 0x80, matching the repaired boot.ini.

## menu.lst (current)

Three entries:

1. **Win2k text-mode setup from RAM ISO (SVBus)** — no swap, El
   Torito chainload, requires F6+SVBus selection.
2. **Boot installed Windows 2000 (requires rdisk(0) in boot.ini)** —
   swap, then chainload `(hd0,0)/ntldr`. Only useful after the
   manual boot.ini repair.
3. **Win2k text-mode setup via direct SETUPLDR.BIN (fallback)** —
   diagnostic alternative to entry 1 if El Torito misbehaves.

See `crates/bootsmith/src/pipeline/windows_2000.rs` for the source-of-
truth strings and inline comments documenting the rationale.

## Phase 3: auto-repair boot.ini

The remaining 1.0 polish item. Detailed plan in
`docs/BACKLOG.md` under "Win2k boot.ini auto-repair (phase 3)". The
manual Recovery Console procedure (Option A above) is the simplest
candidate to automate. Alternatives:

- A small Linux initrd (Tinycore-sized) that mounts NTFS, edits the
  file, reboots. ~20 MB vendor cost.
- FreeDOS + NTFS write tool. Smaller but the FOSS NTFS-write story is
  unmaintained.
- GRUB4DOS raw-sector patch of the MFT entry holding boot.ini's
  resident data. Smallest footprint, most fragile.

Recommendation in the BACKLOG: Tinycore-Linux initrd if we ship phase
3; otherwise ship Win2k as "install completes, manual one-time
boot.ini repair required" in the user docs.

## References

- SVBus repo: <https://github.com/grub4dos/svbus>
- SVBus ReadMe (Win2k recipe): <https://github.com/grub4dos/svbus/blob/master/ReadMe.txt>
- SVBus on SourceForge (V1.3 archive): <https://sourceforge.net/projects/svbus/>
- GRUB4DOS 0.4.5c download (chenall mirror): <http://dl.grub4dos.chenall.net/grub4dos-0.4.5c-2015-05-18.7z>
- GNU GRUB Legacy manual — DOS/Windows section (canonical hd0/hd1 swap recipe): <https://www.gnu.org/software/grub/manual/legacy/DOS_002fWindows.html>
- Grub4dos Guide — Boot Options (Diddy): <http://diddy.boot-land.net/grub4dos/files/boot.htm>
- Grub4dos Guide — Map Command: <http://owl.homeip.net/manuals/systems/dos/grub4dos/files/map.htm>
- chenall/grub4dos issue #157 (BPB / DL chainload failure modes): <https://github.com/chenall/grub4dos/issues/157>
- chenall/grub4dos issue #154 (low-memory regression — investigated but not load-bearing): <https://github.com/chenall/grub4dos/issues/154>
- MS Learn — Edit boot.ini in Windows 2000: <https://learn.microsoft.com/en-us/troubleshoot/windows-client/setup-upgrade-and-drivers/edit-boot-ini-file-windows-2000>
- MSFN — Alternative location of setup files: <https://msfn.org/board/topic/119742-alternative-location-of-setup-files-when-installing-from-hd-media/>
- reboot.pro WinVBlock/FiraDisk thread 8168: <http://reboot.pro/index.php?showtopic=8168>
- reboot.pro FiraDisk thread 8804: <http://reboot.pro/index.php?showtopic=8804>
- RMPrepUSB tutorial 030: <https://rmprepusb.com/tutorials/030-how-to-install-xp-onto-a-hard-disk-from-an-xp-iso-on-a-bootable-usb-drive/>
- Easy2Boot Win2k page (delegates to WinSetupFromUSB): <https://easy2boot.xyz/create-your-website-with-blocks/add-payload-files/windows-install-isos/windows-2000/>
