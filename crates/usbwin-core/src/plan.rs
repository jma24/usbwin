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
    /// XP install USB needs a different boot chain (Grub4DOS-style chainloader
    /// + txtsetup.sif rewriting); it gets its own variant once this works.
    Windows,

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
