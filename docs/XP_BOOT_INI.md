# XP `boot.ini` — design notes & canonical-recipe comparison

Captured 2026-05-20 while debugging a hal.dll fall-through on the
[[project-hardware-rig]] (Dell E6410, Win 7 on internal HDD's
`rdisk(1)partition(1)`).

## The question

Why does our staged `boot.ini` on the USB have a `"2nd, GUI mode setup"`
entry pointing at `multi(0)disk(0)rdisk(1)partition(1)\WINDOWS`? On rigs
with an existing non-XP Windows on that partition, the entry boots Win 7
under XP NTLDR and produces a misleading `<Windows root>\system32\hal.dll
missing`. Should we remove it?

## Where the entry came from

We didn't invent it. It's verbatim from the canonical USB_MultiBoot
recipe (jaclaz, ilko_t, wimb, cdob — boot-land.net / MSFN, 2006–2008).
Mirror: <https://github.com/ruo91/USB_MultiBoot>.

The canonical template (`USB_MultiBoot_10/usb_xpbt/boot.ini`):

```
[Boot Loader]
Timeout=30
Default=multi(0)disk(0)rdisk(1)partition(1)\WINDOWS

[Operating Systems]
C:\btsec\XPSTP.bs="1. Start Windows XP Setup, Never unplug USB-Drive Until setup is done"
multi(0)disk(0)rdisk(1)partition(1)\WINDOWS="2. Continue GUI Mode Setup Windows XP + Start XP from HDD 1" /FASTDETECT
multi(0)disk(0)rdisk(2)partition(1)\WINDOWS="Continue GUI Setup + Start XP from HDD 2, Select if you are installing on HDD 2" /FASTDETECT
c:\grldr="4. Start GRUB4DOS Menu - DOS FPY IMAGES + Linux + XP Rec Cons + Vista setup"
C:\btsec\PELDR.bs="5. BartPE - MINI XP"
C:\btsec\BOOTMGR.bs="6. Windows PE 2.0 - Run Vista Setup from Second Partition of USB-Drive"
C:\btsec\SLBOOT.bs="7. SYSLINUX Menu"
C:\btsec\MSBOOT.bs="8. MS-DOS 7.10"
```

Composition of the menu in the canonical recipe is done at burn time via
`USB_MultiBoot_10.cmd` + `makebt/MakeBS3.cmd`, which appends entries with
a small `fedit` helper and removes any `BOOTMGR` / `SYSLINUX` / `MS-DOS`
strays:

```cmd
FIND "C:\btsec\XPSTP.bs" %usbdrive%\boot.ini >NUL
IF %ERRORLEVEL%==0 (
  CALL makebt\MakeBS3.cmd %usbdrive%\XPSTP
) ELSE (
  CALL makebt\MakeBS3.cmd %usbdrive%\XPSTP /a "1. Begin TXT Mode..."
)
makebt\fedit -f %usbdrive%\boot.ini -rem -l:o BOOTMGR
makebt\fedit -f %usbdrive%\boot.ini -rem -l:o SYSLINUX
makebt\fedit -f %usbdrive%\boot.ini -rem -l:o MS-DOS
```

## What each canonical entry is for

| # | Target                                        | Intent                                                    |
| - | --------------------------------------------- | --------------------------------------------------------- |
| 1 | `C:\btsec\XPSTP.bs` (bootsector)              | Text-mode setup. NTLDR chainloads the XP setup bootsector. |
| 2 | `rdisk(1)partition(1)\WINDOWS` /FASTDETECT    | Continue GUI mode after text-mode reboot + boot the installed XP from USB later. |
| 3 | `rdisk(2)partition(1)\WINDOWS` /FASTDETECT    | Same as #2 but for "I'm installing on HDD 2." Multi-disk users get a working option. |
| 4 | `c:\grldr`                                    | GRUB4DOS menu (the kitchen sink — DOS floppies, Linux, recovery console, Vista). |
| 5 | `C:\btsec\PELDR.bs`                           | BartPE mini-XP boot.                                     |
| 6 | `C:\btsec\BOOTMGR.bs`                         | Vista PE / second partition.                              |
| 7 | `C:\btsec\SLBOOT.bs`                          | SYSLINUX menu.                                            |
| 8 | `C:\btsec\MSBOOT.bs`                          | MS-DOS 7.10.                                              |

Three observations:

1. **Default is entry #2, not #1.** Their model: after text-mode reboot,
   NTLDR auto-continues into GUI mode. The user only sees the menu if
   they want to pick something else.
2. **Entry #2 is dual-purpose**: "Continue GUI Mode Setup … *+ Start XP
   from HDD 1*." Mid-install it continues setup; post-install it boots
   the finished system. The description makes that explicit.
3. **They had the rdisk numbering issue documented** — that's why entry
   #3 exists. They knew BIOS enumeration order varied.

## What we kept, what we changed

Our `boot.ini` (was, before the WIPE.DAT addition):

```ini
[boot loader]
timeout=10
default=C:\$WIN_NT$.~BT\BOOTSECT.DAT

[operating systems]
C:\$WIN_NT$.~BT\BOOTSECT.DAT="1st, text mode setup"
multi(0)disk(0)rdisk(1)partition(1)\WINDOWS="2nd, GUI mode setup"
```

Differences from canonical:

| Aspect          | Canonical                                | usbwin                                |
| --------------- | ---------------------------------------- | ------------------------------------- |
| Timeout         | 30 s                                     | 10 s                                  |
| Default         | `rdisk(1)partition(1)\WINDOWS` (#2)      | text-mode bootsector (#1)             |
| Text-mode path  | `C:\btsec\XPSTP.bs`                      | `C:\$WIN_NT$.~BT\BOOTSECT.DAT`        |
| GUI entry args  | `/FASTDETECT`                            | (none)                                |
| Multi-disk      | rdisk(1) AND rdisk(2) entries            | rdisk(1) only                         |
| Extras          | GRUB4DOS, BartPE, Vista PE, SYSLINUX, DOS | none                                  |
| Scope           | Kitchen-sink multi-boot                  | Focused XP installer                  |

## The dual-boot footgun

The canonical recipe assumes a **virgin target** — fresh disk, XP about
to be installed onto `rdisk(1)partition(1)`. Once setup completes,
entry #2 boots the finished install. Reasonable.

On a rig with a pre-existing non-XP Windows on `rdisk(1)partition(1)`,
that entry tries to boot the pre-existing OS under XP's NTLDR. XP NTLDR
can't load Win 7's HAL → `hal.dll missing`. The error is misleading: HAL
exists, but it's the wrong HAL for the loader. Cost us ~30 minutes of
debugging.

Note that picking entry #2 mid-install is *never* correct anyway: the
post-text-mode GUI-mode continuation is supposed to come from the HDD's
own `boot.ini` (written by text-mode setup, points at the in-progress
install). The USB's `boot.ini` entry #2 is a *post-install* convenience
that only works on a virgin target.

## Options going forward

Listed in increasing scope.

### A. Drop entry #2 entirely

Single-entry `boot.ini`, defaults to text-mode setup. Loses the
"boot installed XP from USB without unplugging" convenience — which we
have no evidence anyone has ever used successfully.

- **Pro**: zero footgun, simplest design, matches our focused-installer scope.
- **Con**: deviates from canonical recipe (which is arguably the point —
  the canonical is kitchen-sink, we are not).

### B. Mirror the canonical multi-disk handling

Keep entry #2, add a `rdisk(2)` entry like the canonical does. Helps
users with multiple HDDs to find a working entry.

- **Pro**: faithful to canonical intent.
- **Con**: doesn't help the dual-boot-on-rdisk(1) case (which is ours).
  Still a guessing game for the user.

### C. Auto-target via CLI flag

`--xp-target-disk=rdisk1` (or auto-detect by enumerating SATA disks via
`diskutil` and asking). Emit a tailored `boot.ini`.

- **Pro**: actually correct for arbitrary hardware.
- **Con**: most code; auto-detect from macOS-side can't predict BIOS
  enumeration order with certainty.

### D. The WIPE.DAT route (newly added entry #3)

Entry #3 (added 2026-05-20) chainloads `\WIPE.DAT`, an mkmsbr-supplied
bootsector that zeros the first 1 MiB of the non-USB primary disk. On
a dirty target rig (existing Windows, GPT remnants, etc.), this
*converts* the rig into a virgin target on demand. After running it,
entries #1 and #2 behave as the canonical recipe assumes.

This is orthogonal to A/B/C — it changes the *target*, not the boot.ini
shape. Combined with **A** it's especially clean: text-mode entry as
default + wipe entry for "I'm sure, nuke it first" = no footgun.

## Recommendation context

If WIPE.DAT works as designed on hardware, **A + WIPE.DAT** is the
strongest combination:

- One install path (text-mode), no menu confusion mid-install.
- One escape valve (wipe + reboot + try again) for dirty-target cases.
- No `rdisk()` hardcoding to be wrong about.

If WIPE.DAT doesn't pan out, **B** matches what experienced
WinSetupFromUSB users expect; **C** is the "do it right" version that
requires more code than the rest of this combined.

## References

- USB_MultiBoot mirror: <https://github.com/ruo91/USB_MultiBoot>
  - Template: `USB_MultiBoot_10/usb_xpbt/boot.ini`
  - Composer: `USB_MultiBoot_10/USB_MultiBoot_10.cmd`
  - Per-entry append helper: `USB_MultiBoot_10/makebt/MakeBS3.cmd`
- Our boot.ini: `crates/usbwin/src/pipeline/xp_staging.rs` (`BOOT_INI`)
- Tech-debt entry #6: `docs/TECH_DEBT.md`
- Hardware rig context: [[project-hardware-rig]] memory
