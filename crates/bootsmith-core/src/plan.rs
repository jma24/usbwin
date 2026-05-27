//! `WritePlan` is the typed, fully-resolved description of "what we are going
//! to do to this device." Building a plan is deterministic and pure; executing
//! it is the only step that needs a real `Device`.
//!
//! The plan is the unit that gets dry-run, golden-tested, and executed.

use std::path::PathBuf;

/// The four boot-record families bootsmith understands. Resolved from
/// `ModeRequest::Auto` via `bootsmith-iso` inspection, or supplied directly by
/// the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    /// Raw write of a hybrid ISO9660 image. Most modern Linux/BSD distros.
    Hybrid,

    /// MBR + active FAT32 + bootmgr-loading PBR + file copy. Win 7 through 11.
    Windows,

    /// NT-family XP/2003-style install USB using GRUB4DOS + FiraDisk:
    /// RAM-map the original ISO as a virtual CD, expose it to protected-mode
    /// setup with FiraDisk, and drive-swap so the internal HDD is first.
    WindowsNtXp,

    /// Windows 2000 (NT 5.0) install USB. Same GRUB4DOS + RAM-mapped ISO
    /// chain as [`WindowsNtXp`], but with SVBus in place of FiraDisk —
    /// FiraDisk's SCSI miniport collides with the NT 5.0 storage stack
    /// (0x7B INACCESSIBLE_BOOT_DEVICE / 0xC0000034). See
    /// docs/WIN2K_SVBUS.md.
    Windows2000,

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
            BootMode::Windows2000 => "windows-2000",
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
