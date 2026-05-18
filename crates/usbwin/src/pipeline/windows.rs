//! Windows 7+ install USB pipeline. This is the gap UNetbootin leaves
//! (FIELD_FINDINGS §4) and the reason usbwin exists.
//!
//! Step sequence (matches the validated manual recipe in FIELD_FINDINGS §8,
//! adapted to use native primitives where possible):
//!
//!   1. Unmount the whole disk.
//!   2. Write the MBR with one active FAT32-LBA primary partition.
//!      Partition starts at LBA 2048, extends to end of disk.
//!   3. Re-read the device so the kernel notices the new partition table
//!      (diskutil unmount + mount cycle).
//!   4. Format /dev/rdiskNs1 as FAT32 via newfs_msdos.
//!   5. Mount /dev/diskN so we can cp files in.
//!   6. Attach the Win 7 ISO via hdiutil.
//!   7. Copy the ISO's contents file-by-file to the FAT32 USB. Single
//!      fsync at the end; no per-file fsync.
//!   8. Detach the ISO.
//!   9. Unmount the USB.
//!   10. Splice our FAT32 PBR onto sector 0 of the partition (preserving
//!       the BPB that newfs_msdos wrote).
//!   11. Eject.

use anyhow::{anyhow, bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::time::Duration;

use usbwin_core::{Device, WritePlan};
use usbwin_disk::raw::{OpenMode, RawDevice};
use usbwin_disk::DeviceInfo;

use super::diskutil;

const SECTOR_SIZE: u64 = 512;

pub fn run(plan: &WritePlan, info: &DeviceInfo, verify: bool) -> Result<()> {
    let bsd_path = info.path.replace("/dev/r", "/dev/");
    let partition_raw = format!("{}s1", info.path);

    // 1. Unmount everything mounted from this disk.
    diskutil::unmount_disk(&bsd_path).context("unmount before partition write")?;

    // 2. Write the MBR.
    write_mbr_sector(info)?;

    // 3. Kernel needs to re-read the partition table. The simplest portable
    //    way on macOS is unmount + immediate mount cycle.
    let _ = diskutil::unmount_disk(&bsd_path);
    diskutil::mount_disk(&bsd_path).context("mount after partition write")?;
    let _ = diskutil::unmount_disk(&bsd_path); // unmount again before format

    // 4. Format the partition.
    diskutil::newfs_msdos_fat32(&partition_raw, &plan.label)
        .with_context(|| format!("formatting {partition_raw} as FAT32"))?;

    // 5. Mount the freshly-formatted partition so we can copy files into it.
    diskutil::mount_disk(&bsd_path).context("mount after format")?;
    let usb_mount = find_mount_for_label(&plan.label)
        .ok_or_else(|| anyhow!("formatted partition didn't appear in /Volumes"))?;

    // 6. Attach the ISO.
    let iso_mount = diskutil::hdiutil_attach_iso(&plan.iso_path)
        .with_context(|| format!("attaching ISO {}", plan.iso_path.display()))?;

    // 7. Copy ISO contents -> USB.
    let copy_result = copy_iso_contents(&iso_mount, &usb_mount);

    // 8. Detach the ISO regardless of copy outcome.
    let _ = diskutil::hdiutil_detach(&iso_mount);
    copy_result.context("copying ISO contents to USB")?;

    // 9. Unmount the USB so we can write the PBR.
    diskutil::unmount_disk(&bsd_path).context("unmount before PBR splice")?;

    // 10. Splice the FAT32 PBR onto sector 0 of the partition, preserving
    //     the BPB that newfs_msdos wrote.
    splice_pbr(&partition_raw, &info.model, verify).context("PBR splice")?;

    // 11. Unmount + eject.
    let _ = diskutil::unmount_disk(&bsd_path);
    diskutil::eject(&bsd_path).context("eject after write")?;

    println!(
        "\nusbwin: {} -> {} (Windows 7+ mode) OK",
        plan.iso_path.display(),
        info.path
    );
    Ok(())
}

fn write_mbr_sector(info: &DeviceInfo) -> Result<()> {
    if usbwin_boot::MBR_BOOT.is_empty() {
        bail!(
            "MBR boot blob not embedded; rebuild with --features usbwin-boot/embed-boot-asm"
        );
    }
    let disk_sectors = info.size_bytes / SECTOR_SIZE;
    let mbr = usbwin_boot::build_mbr(usbwin_boot::MBR_BOOT, disk_sectors)
        .map_err(|e| anyhow!("building MBR: {e}"))?;

    let mut dev = RawDevice::open(&info.path, OpenMode::ReadWrite, &info.model)
        .context("opening whole disk for MBR write")?;
    dev.write_at(0, &mbr).map_err(anyhow_from_core)?;
    dev.sync().map_err(anyhow_from_core)?;

    // Verify the MBR landed.
    let mut readback = vec![0u8; 512];
    dev.read_at(0, &mut readback).map_err(anyhow_from_core)?;
    if readback != mbr {
        bail!("MBR write verify mismatch");
    }
    Ok(())
}

fn splice_pbr(partition_raw: &str, model: &str, verify: bool) -> Result<()> {
    if usbwin_boot::FAT32_PBR_BOOT.is_empty() {
        bail!(
            "FAT32 PBR boot blob not embedded; rebuild with --features usbwin-boot/embed-boot-asm"
        );
    }
    let mut dev = RawDevice::open(partition_raw, OpenMode::ReadWrite, model)
        .with_context(|| format!("opening {partition_raw} for PBR splice"))?;

    // Read the freshly-formatted PBR (carries the BPB from newfs_msdos).
    let mut existing = [0u8; 512];
    dev.read_at(0, &mut existing).map_err(anyhow_from_core)?;

    let spliced = usbwin_boot::splice_fat32_pbr(&existing, usbwin_boot::FAT32_PBR_BOOT)
        .map_err(|e| anyhow!("splice_fat32_pbr: {e}"))?;

    dev.write_at(0, &spliced).map_err(anyhow_from_core)?;
    dev.sync().map_err(anyhow_from_core)?;

    if verify {
        let mut readback = [0u8; 512];
        dev.read_at(0, &mut readback).map_err(anyhow_from_core)?;
        if readback != spliced {
            bail!("PBR splice verify mismatch");
        }
    }
    Ok(())
}

fn find_mount_for_label(label: &str) -> Option<PathBuf> {
    // After a `diskutil mountDisk`, the new volume shows up under /Volumes
    // with its label as the directory name. Wait briefly for it to appear.
    let target = PathBuf::from("/Volumes").join(label);
    for _ in 0..20 {
        if target.exists() {
            return Some(target);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn copy_iso_contents(iso_mount: &Path, usb_mount: &Path) -> Result<()> {
    let entries = walk_files(iso_mount)?;
    let total_bytes: u64 = entries.iter().map(|e| e.size).sum();
    let total_files = entries.len() as u64;

    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::with_template(
            "{prefix:>10}  {bar:40.cyan/blue} {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta}, {pos}/{len} bytes, {msg})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    pb.set_prefix("copying");
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message(format!("0/{total_files} files"));

    // Create directories first so per-file writes don't have to mkdir.
    for entry in &entries {
        if entry.is_dir {
            let dest = usb_mount.join(entry.rel.as_path());
            std::fs::create_dir_all(&dest)
                .with_context(|| format!("mkdir {}", dest.display()))?;
        }
    }

    let mut bytes_copied = 0u64;
    let mut files_copied = 0u64;
    for entry in &entries {
        if entry.is_dir {
            continue;
        }
        let src = iso_mount.join(entry.rel.as_path());
        let dest = usb_mount.join(entry.rel.as_path());
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::copy(&src, &dest)
            .with_context(|| format!("copy {} -> {}", src.display(), dest.display()))?;
        bytes_copied += entry.size;
        files_copied += 1;
        pb.set_position(bytes_copied);
        pb.set_message(format!("{files_copied}/{total_files} files"));
    }

    // Single sync to flush the FS cache. macOS's USB stack will then take
    // care of pushing bytes to flash when we unmount/eject.
    let _ = std::process::Command::new("sync").status();

    pb.finish_with_message(format!("{files_copied}/{total_files} files"));
    Ok(())
}

struct CopyEntry {
    rel: PathBuf,
    size: u64,
    is_dir: bool,
}

fn walk_files(root: &Path) -> Result<Vec<CopyEntry>> {
    let mut out = Vec::new();
    walk_recursive(root, root, &mut out)?;
    out.sort_by(|a, b| a.is_dir.cmp(&b.is_dir).reverse().then(a.rel.cmp(&b.rel)));
    Ok(out)
}

fn walk_recursive(root: &Path, dir: &Path, out: &mut Vec<CopyEntry>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("read_dir {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|e| anyhow!("strip_prefix: {e}"))?
            .to_path_buf();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            out.push(CopyEntry {
                rel: rel.clone(),
                size: 0,
                is_dir: true,
            });
            walk_recursive(root, &path, out)?;
        } else if metadata.is_file() {
            out.push(CopyEntry {
                rel,
                size: metadata.len(),
                is_dir: false,
            });
        }
        // Skip symlinks, devices, etc. - Win 7 ISOs don't contain them.
    }
    Ok(())
}

fn anyhow_from_core(e: usbwin_core::Error) -> anyhow::Error {
    anyhow!("{e}")
}
