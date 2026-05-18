//! Thin wrappers around `diskutil` for unmount/mount/eject. The only
//! shell-out usbwin does for control-plane work; the data plane is all
//! native via RawDevice.

use anyhow::{bail, Context, Result};
use std::process::Command;

pub fn unmount_disk(bsd_path: &str) -> Result<()> {
    run_diskutil(&["unmountDisk", bsd_path])
        .with_context(|| format!("diskutil unmountDisk {bsd_path}"))
}

#[allow(dead_code)]
pub fn mount_disk(bsd_path: &str) -> Result<()> {
    run_diskutil(&["mountDisk", bsd_path])
        .with_context(|| format!("diskutil mountDisk {bsd_path}"))
}

pub fn eject(bsd_path: &str) -> Result<()> {
    run_diskutil(&["eject", bsd_path])
        .with_context(|| format!("diskutil eject {bsd_path}"))
}

fn run_diskutil(args: &[&str]) -> Result<()> {
    let output = Command::new("diskutil")
        .args(args)
        .output()
        .with_context(|| format!("spawning `diskutil {}`", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "diskutil {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
