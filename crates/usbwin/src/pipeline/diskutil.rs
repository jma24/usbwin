//! Thin wrappers around macOS control-plane tools: `diskutil`,
//! `newfs_msdos`, and `hdiutil`. These are the only shell-outs in usbwin;
//! the data plane (raw I/O, byte assembly, verify) is all native.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
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

/// Format a partition (e.g. `/dev/rdisk6s1`) as FAT32 with the given
/// volume label. Uses macOS's built-in `newfs_msdos`.
///
/// IMPORTANT: takes the **partition** device (`disk6s1`), not the whole
/// disk (`disk6`). The MBR must already have been written so the partition
/// exists.
pub fn newfs_msdos_fat32(partition_path: &str, label: &str) -> Result<()> {
    let output = Command::new("newfs_msdos")
        .args(["-F", "32", "-v", label, partition_path])
        .output()
        .with_context(|| format!("spawning newfs_msdos for {partition_path}"))?;
    if !output.status.success() {
        bail!(
            "newfs_msdos failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Attach an ISO file as a read-only loopback. Returns the temporary mount
/// point that hdiutil chose (e.g. `/Volumes/disk_image_NN`).
///
/// We use `-nobrowse` so Finder doesn't pop up a window for the mounted
/// ISO. We don't pin the mount point; macOS picks a fresh one each time,
/// which is fine since we read it back from hdiutil's output.
pub fn hdiutil_attach_iso(iso: &Path) -> Result<PathBuf> {
    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly"])
        .arg(iso)
        .output()
        .with_context(|| format!("hdiutil attach {}", iso.display()))?;
    if !output.status.success() {
        bail!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mount_point = stdout
        .lines()
        .find_map(|l| {
            l.split_whitespace()
                .find(|p| p.starts_with("/Volumes/"))
                .map(PathBuf::from)
        })
        .ok_or_else(|| anyhow!("hdiutil attach gave no /Volumes/ mount: {stdout}"))?;
    Ok(mount_point)
}

/// Detach an ISO mount point that came from `hdiutil_attach_iso`.
pub fn hdiutil_detach(mount_point: &Path) -> Result<()> {
    let output = Command::new("hdiutil")
        .arg("detach")
        .arg(mount_point)
        .output()
        .with_context(|| format!("hdiutil detach {}", mount_point.display()))?;
    if !output.status.success() {
        bail!(
            "hdiutil detach failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
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
