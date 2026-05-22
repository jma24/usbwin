# Windows 2000 install via GRUB4DOS + SVBus

Scoping doc for the Win2k 1.0 blocker. The verified XP path uses
GRUB4DOS + FiraDisk + RAM-mapped ISO; Win2k cannot reuse FiraDisk because
the FiraDisk SCSI miniport is XP/2003+ only and does not enumerate under
NT 5.0. The plan below adds a parallel `windows-2000` mode that keeps the
GRUB4DOS shell but substitutes SVBus as the ramdisk driver.

## Background

Web research summary (2026-05-21):

- **FiraDisk 0.0.1.30 is XP/2003-and-later only.** Sources: reboot.pro
  threads 8168 (WinVBlock/FiraDisk/RamDisks) and 8804 (FiraDisk release
  notes — no Win2k entry in the changelog). The FiraDisk INF lacks
  version decoration in `[Manufacturer]`, so it will *attempt* install on
  Win2k, but the SCSI miniport is not validated against NT 5.0's storage
  stack.
- **SVBus** is the actively maintained successor to WinVBlock, hosted by
  the grub4dos org at github.com/grub4dos/svbus. Its `ReadMe.txt`
  documents Win2k SP4 install end-to-end with a verbatim `menu.lst`
  recipe and F6 procedure. SourceForge mirror confirms "Windows 2000 up
  to the newest Windows 10".
- **Most likely BSOD** for the 2026-05-21 hardware test:
  `STOP 0x7B INACCESSIBLE_BOOT_DEVICE`, first arg `0xC0000034`
  STATUS_OBJECT_NAME_NOT_FOUND = no boot device enumerated after the
  setupldr real-mode → protected-mode handoff. Exactly the failure the
  ramdisk driver exists to solve. **Not yet validated — needs the
  diagnostic capture below.**
- **USB controller drivers (`usbehci.sys`/`usbohci.sys`) probably do
  NOT matter** for the usbwin RAM-mapped-ISO architecture. Follow-up
  research (2026-05-21) found that the MSFN 147119 BSOD applies to
  WinSetupFromUSB's sector-mapped path where setup keeps reading from
  USB throughout text-mode; once GRUB4DOS `map --mem` finishes, INT 13h
  is serviced from the RAM buffer and the USB stick can be physically
  unplugged (RMPrepUSB tutorial 030, MSFN thread 137714). **This needs
  hardware validation on the Win2k path before we treat it as settled.**
  If validation confirms, the `txtsetup.sif` USB patch can be dropped.

## Diagnostic capture on next hardware run

Before changing any code, capture the actual failure to confirm the
hypothesis:

1. Boot the existing Win2k USB on the E6410.
2. When the BSOD appears, photograph the top line:
   `*** STOP: 0xXXXXXXXX (0xAAAAAAAA, 0xBBBBBBBB, 0xCCCCCCCC, 0xDDDDDDDD)`.
   Diagnostic gold is the first parenthesised arg when the stop is 0x7B:
   - `0xC0000034` = no boot device enumerated (driver problem).
   - `0xC000000E` = device gone after handoff.
   - `0xC0000010` = device unsupported.
3. Append `/sos` to the setupldr command line in the GRUB4DOS entry
   (`chainloader /i386/setupldr.bin /sos`). Win2k honours `/sos` and
   prints each driver name as it loads, so the last name on screen
   before the BSOD identifies the culprit.
4. Serial console (`/debug /debugport=com1 /baudrate=115200`) works but
   requires KD on the other end — skip unless steps 1-3 are
   inconclusive.

## Implementation plan

### 1. Vendor SVBus

- Pin a specific grub4dos/svbus release tag.
- Drop the `svbus.ima` floppy image and the unpacked `svbus.sys` +
  `txtsetup.oem` + INF into `crates/usbwin/src/pipeline/win2k_assets/`
  (parallel to the existing FiraDisk asset layout).
- License/provenance note in the asset directory README.

### 2. New `windows-2000` pipeline mode

- Add `Mode::Windows2000` (or extend `Mode::WindowsNtxp` with a Win2k
  variant — TBD during implementation).
- ISO classifier already labels Win2k as NT5-class; wire the
  classification to the new mode when `--type=auto` matches Win2k.
- Reuse the XP GRUB4DOS shape:
  - Same `grldr.mbr` boot track via mkmsbr.
  - Same `map --mem <ISO> (0xff)` for the RAM-mapped install media.
  - Substitute `map --mem (md)0x800+0x16+... (fd0)` to point at
    `SVBUS.IMA` instead of `FIRADISK.IMA`.
  - Same chainloader handoff to `(0xff)/I386/SETUPLDR.BIN`.
- Stage SVBus into the ISO's `I386` directory the way FiraDisk is staged
  for XP: `txtsetup.oem` reference, driver INF, `svbus.sys` copy.
- The Win2k `WINNT.SIF` references `"SVBus Virtual SCSI Host Adapter
  x86"` as the first SCSI device under `[MassStorageDrivers]` /
  `[OEMBootFiles]`.

### 3. `txtsetup.sif` USB-driver patch (probably not needed — validate)

Follow-up research suggests the `usbehci.sys` / `usbohci.sys`
missing-file BSOD is specific to the sector-mapped WinSetupFromUSB
path and does not apply when GRUB4DOS RAM-maps the full ISO. Skip this
step in the initial implementation; if Win2k still BSODs after SVBus
swap-in and the BSOD signature matches MSFN thread 147119 rather than
0x7B/0xC0000034, reintroduce the patch then. Validation gate: capture
the stop code on the next E6410 run before deciding.

### 4. Unattended

Win2k unattended uses the same `WINNT.SIF` mechanism as XP (the same
`[Unattended]` / `[UserData]` / `[GuiUnattended]` sections). The existing
SIF generator in the XP path should mostly carry over; differences to
audit:

- Win2k product-key format vs XP.
- `[Display]` defaults Win2k accepts.
- Any sections Win2k rejects that XP tolerates.

### 5. Hardware verification

Done means an unattended Win2k SP4 install reaches first desktop boot on
the Dell E6410 from a usbwin-burned USB, with BIOS SATA in ATA mode (the
AHCI item is a separate 1.0 blocker shared with XP).

## Out of scope for this work

- AHCI / F6 storage drivers for Win2k — covered by the existing "XP AHCI/
  SATA/RAID textmode storage support" 1.0 blocker, will extend to Win2k
  once both modes share the F6 driver path.
- Win2k under QEMU in the `usbwin-eval` harness — useful but not on the
  critical path; add after hardware-green.

## References

- SVBus repo: https://github.com/grub4dos/svbus
- SVBus ReadMe (Win2k recipe): https://github.com/grub4dos/svbus/blob/master/ReadMe.txt
- SVBus on SourceForge: https://sourceforge.net/projects/svbus/
- reboot.pro WinVBlock/FiraDisk thread 8168: http://reboot.pro/index.php?showtopic=8168
- reboot.pro FiraDisk thread 8804: http://reboot.pro/index.php?showtopic=8804
- MSFN Win2k-from-USB BSOD thread 147119: https://msfn.org/board/topic/147119-win-setup-from-usb-installing-windows-2000-win2k-from-usb-bsod-us/ (sector-mapped path, not our RAM-map architecture)
- MSFN RAM-loaded XP ISO thread 137714: https://msfn.org/board/topic/137714-install-xp-from-a-ram-loaded-iso-image/ (basis for the "USB drivers irrelevant after map --mem" claim — still needs hardware validation on Win2k)
- RMPrepUSB tutorial 030: https://rmprepusb.com/tutorials/030-how-to-install-xp-onto-a-hard-disk-from-an-xp-iso-on-a-bootable-usb-drive/
- Easy2Boot Win2k page: https://easy2boot.xyz/create-your-website-with-blocks/add-payload-files/windows-install-isos/windows-2000/
