//! Windows XP install USB pipeline (v0.3 work-in-progress).
//!
//! XP install media (i386/-style, NTLDR-based boot) is fundamentally
//! different from Vista+/Win 7+. It was designed for CD/floppy in 2001 and
//! does NOT boot from USB cleanly without the modifications established by
//! the WinSetupFromUSB project circa 2007-2009. The two essential mods:
//!
//! 1. `txtsetup.sif` must declare USB drivers (`usbehci`, `usbohci`,
//!    `usbuhci`, `usbhub`, `usbstor`) in the `[BootBusExtenders.Load]`
//!    section instead of `[InputDevicesSupport.Load]`. Without this, text-
//!    mode setup loads the USB devices only as input devices, can't see
//!    the install source as mass storage, and the second-stage reboot
//!    fails to find the install files.
//!
//! 2. Optionally inject `WaitBT.sys` and `Wait4UFD.sys` to delay the OS
//!    until the BIOS finishes initializing the USB bus on the target
//!    machine, avoiding the post-reboot `0x7B INACCESSIBLE_BOOT_DEVICE`
//!    BSOD on some hardware.
//!
//! Boot record bytes come from `ms-sys --mbr` (Win 2000/XP/2003 MBR) and
//! `ms-sys --fat32nt` (NT 5.x-style NTLDR-loading FAT32 PBR), the XP
//! analogues of the `--mbr7` / `--fat32pe` pair used by the Win 7+ path.
//!
//! Chunk status (see docs/V0.3_WINDOWS_XP.md):
//!   1. BootMode + CLI + dispatch                              DONE
//!   2. Partition / format / file copy (clone of windows.rs)   THIS COMMIT
//!   3. txtsetup.sif parser + modifier                         TODO
//!   4. Wire SIF modification into file-copy                   TODO
//!   5. ms-sys --mbr + --fat32nt invocation                    TODO
//!   6. WaitBT/Wait4UFD driver injection                       TODO
//!   7. winnt.sif (unattended answers) generator               TODO

use anyhow::{bail, Result};

use usbwin_core::WritePlan;
use usbwin_disk::DeviceInfo;

pub fn run(_plan: &WritePlan, _info: &DeviceInfo, _verify: bool) -> Result<()> {
    bail!(
        "Windows XP install mode is v0.3 work-in-progress. \
         The chunks landing in this commit are: BootMode enum, CLI dispatch, \
         label defaulting. The actual pipeline (partition, format, file \
         copy, ms-sys boot records, txtsetup.sif modification, optional \
         driver injection) lands in subsequent commits. \
         See docs/V0.3_WINDOWS_XP.md for the plan."
    )
}
