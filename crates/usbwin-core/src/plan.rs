//! `WritePlan` is the typed, fully-resolved description of "what we are going
//! to do to this device." Building a plan is deterministic and pure; executing
//! it is the only step that needs a real `Device`.
//!
//! The plan is the unit that gets dry-run, golden-tested, and executed.

use std::path::PathBuf;

/// The four boot-record families usbwin understands. Resolved from
/// `ModeRequest::Auto` via `usbwin-iso` inspection, or supplied directly by
/// the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    /// Raw write of a hybrid ISO9660 image. Most modern Linux/BSD distros.
    Hybrid,

    /// MBR + active FAT32 + bootmgr-loading PBR + file copy. Win 7 through 11.
    Windows,

    /// NT-family XP/2000-style install USB using GRUB4DOS + FiraDisk:
    /// RAM-map the original ISO as a virtual CD, expose it to protected-mode
    /// setup with FiraDisk, and drive-swap so the internal HDD is first.
    WindowsNtXp,

    /// Legacy XP install USB. NTLDR-based boot chain (`ms-sys --mbr` + `--fat32nt`),
    /// FAT32 file copy from i386/-style install media, plus the WinSetupFromUSB
    /// modifications: txtsetup.sif edited to move USB drivers into
    /// `BootBusExtenders.Load`, optional WaitBT/Wait4UFD waiter injection,
    /// optional winnt.sif answer file. v0.3 work-in-progress; see
    /// docs/V0.3_WINDOWS_XP.md.
    WindowsXp,

    /// MBR + active FAT32 + syslinux boot code + file copy. Older Linux ISOs
    /// that aren't hybrid (e.g. some isolinux-only distros).
    IsolinuxLinux,

    /// GPT + ESP + EFI directory copy. Modern UEFI-only installers.
    UefiOnly,
}

impl BootMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            BootMode::Hybrid => "hybrid",
            BootMode::Windows => "windows",
            BootMode::WindowsNtXp => "windows-ntxp",
            BootMode::WindowsXp => "windows-xp-legacy",
            BootMode::IsolinuxLinux => "linux",
            BootMode::UefiOnly => "uefi",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WritePlan {
    pub iso_path: PathBuf,
    pub iso_bytes: u64,
    pub mode: BootMode,
    pub label: String,
}
