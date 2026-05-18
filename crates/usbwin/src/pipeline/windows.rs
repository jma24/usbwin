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
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    let total_files = entries.iter().filter(|e| !e.is_dir).count() as u64;

    // Two parallel bars. install.wim dominates the byte count and finishes
    // long before the thousands of small files do; one bar can't honestly
    // represent both. Show both so the user sees what's actually happening.
    let multi = MultiProgress::new();
    let bar_style = ProgressStyle::with_template(
        "  {prefix:<6} {wide_bar:.cyan/blue} {msg}",
    )
    .unwrap()
    .progress_chars("█▓▒░ ");

    let pb_bytes = multi.add(ProgressBar::new(total_bytes));
    pb_bytes.set_style(bar_style.clone());
    pb_bytes.set_prefix("bytes");
    pb_bytes.enable_steady_tick(Duration::from_millis(100));

    let pb_files = multi.add(ProgressBar::new(total_files));
    pb_files.set_style(bar_style);
    pb_files.set_prefix("files");
    pb_files.enable_steady_tick(Duration::from_millis(100));

    // Pre-create directories so per-file copies don't have to mkdir each
    // time. Per FIELD_FINDINGS, the per-file overhead is the bottleneck;
    // anything we hoist out of the inner loop helps.
    for entry in &entries {
        if entry.is_dir {
            let dest = usb_mount.join(entry.rel.as_path());
            std::fs::create_dir_all(&dest)
                .with_context(|| format!("mkdir {}", dest.display()))?;
        }
    }

    let start = Instant::now();
    let mut bytes_copied = 0u64;
    let mut files_copied = 0u64;
    for entry in &entries {
        if entry.is_dir {
            continue;
        }
        let src = iso_mount.join(entry.rel.as_path());
        let dest = usb_mount.join(entry.rel.as_path());

        // Chunked copy with intra-file progress updates. std::fs::copy is
        // tempting (one syscall, uses macOS copyfile) but for files larger
        // than a few hundred MB the bar freezes for the whole copy duration
        // and re-animates with a sudden jump at the end. Win 7 install.wim
        // is 2+ GB = a one-minute freeze. Bad UX.
        copy_chunked(
            &src,
            &dest,
            &pb_bytes,
            &pb_files,
            &mut bytes_copied,
            total_bytes,
            files_copied,
            total_files,
            start,
        )
        .with_context(|| format!("copy {} -> {}", src.display(), dest.display()))?;

        files_copied += 1;
        update_messages(
            &pb_bytes,
            &pb_files,
            bytes_copied,
            total_bytes,
            files_copied,
            total_files,
            start,
        );
    }

    // Single sync at the end. Per PERFORMANCE.md, per-file fsync is the
    // UNetbootin trap that took it to 2 files/sec.
    let _ = std::process::Command::new("sync").status();

    pb_bytes.finish();
    pb_files.finish();
    Ok(())
}

/// Copy a single file in 4 MiB chunks, updating the byte progress bar
/// between chunks so the user sees motion during multi-GB writes.
#[allow(clippy::too_many_arguments)]
fn copy_chunked(
    src: &Path,
    dest: &Path,
    pb_bytes: &ProgressBar,
    pb_files: &ProgressBar,
    bytes_copied: &mut u64,
    total_bytes: u64,
    files_copied: u64,
    total_files: u64,
    start: Instant,
) -> Result<()> {
    const CHUNK: usize = 4 * 1024 * 1024;
    let mut src_f = File::open(src).with_context(|| format!("open {}", src.display()))?;
    let mut dest_f = File::create(dest).with_context(|| format!("create {}", dest.display()))?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = src_f.read(&mut buf).context("read from ISO")?;
        if n == 0 {
            break;
        }
        dest_f
            .write_all(&buf[..n])
            .context("write to USB")?;
        *bytes_copied += n as u64;
        update_messages(
            pb_bytes,
            pb_files,
            *bytes_copied,
            total_bytes,
            files_copied,
            total_files,
            start,
        );
    }
    Ok(())
}

fn update_messages(
    pb_bytes: &ProgressBar,
    pb_files: &ProgressBar,
    bytes_copied: u64,
    total_bytes: u64,
    files_copied: u64,
    total_files: u64,
    start: Instant,
) {
    let elapsed = start.elapsed().as_secs_f64().max(0.01);
    let bps = bytes_copied as f64 / elapsed;
    let fps = files_copied as f64 / elapsed;
    let bytes_eta = if bps > 0.0 {
        (total_bytes - bytes_copied) as f64 / bps
    } else {
        0.0
    };
    let files_eta = if fps > 0.0 {
        (total_files - files_copied) as f64 / fps
    } else {
        0.0
    };
    let eta = bytes_eta.max(files_eta);

    pb_bytes.set_position(bytes_copied);
    pb_bytes.set_message(format!(
        "{:>10} / {:<10} @ {:>10}/s  ETA {}",
        human_bytes(bytes_copied),
        human_bytes(total_bytes),
        human_bytes(bps as u64),
        human_secs(eta as u64),
    ));

    pb_files.set_position(files_copied);
    pb_files.set_message(format!(
        "{:>10} / {:<10} @ {:>8.0}/s  ETA {}",
        files_copied,
        total_files,
        fps,
        human_secs(eta as u64),
    ));
}

fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn human_secs(s: u64) -> String {
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}s", s)
    }
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
