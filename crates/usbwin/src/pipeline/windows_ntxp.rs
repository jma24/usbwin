//! Windows NT 5.x-style install USB pipeline.
//!
//! This is the hardware-green XP path from the recovery plan:
//! GRUB4DOS RAM-maps the original ISO as a virtual CD, maps a FiraDisk
//! textmode-driver floppy, swaps BIOS disk order so the internal HDD is first,
//! and then chainloads XP setup. The second GRUB4DOS menu entry repeats the
//! maps and chainloads the target HDD so GUI-mode setup still sees the CD.

use anyhow::{anyhow, bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use usbwin_core::{Config, Device, WritePlan};
use usbwin_disk::raw::{OpenMode, RawDevice};
use usbwin_disk::DeviceInfo;

use super::diskutil;

const SECTOR_SIZE: u64 = 512;
const PARTITION_START_LBA: u32 = 2048;

const GRLDR: &[u8] = include_bytes!("ntxp_assets/grldr");
const GRLDR_MBR: &[u8] = include_bytes!("ntxp_assets/grldr.mbr");
const FIRADISK_IMA: &[u8] = include_bytes!("ntxp_assets/firadisk.ima");

const MENU_LST: &str = r#"timeout 10
default 0

title 1. XP text-mode setup from RAM ISO (FiraDisk)
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /FIRADISK.IMA (fd0)
map --mem /FIRADISK.IMA (fd1)
map --mem /XP.ISO (0xff)
map --hook
root (0xff)
chainloader (0xff)/I386/SETUPLDR.BIN

title 2. Continue XP GUI-mode setup from internal HDD
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /FIRADISK.IMA (fd0)
map --mem /FIRADISK.IMA (fd1)
map --mem /XP.ISO (0xff)
map --hook
root (hd0,0)
chainloader (hd0)+1

title 3. XP text-mode setup via ISO boot image
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /FIRADISK.IMA (fd0)
map --mem /FIRADISK.IMA (fd1)
map --mem /XP.ISO (0xff)
map --hook
chainloader (0xff)
"#;

pub fn run(plan: &WritePlan, info: &DeviceInfo, config: &Config) -> Result<()> {
    let bsd_path = info.path.replace("/dev/r", "/dev/");
    let partition_raw = format!("{}s1", info.path);

    validate_assets()?;
    validate_capacity(plan, info)?;

    diskutil::unmount_disk(&bsd_path).context("unmount before GRUB4DOS MBR write")?;
    write_grub4dos_mbr_track(info, config.verify).context("writing GRUB4DOS MBR/boot track")?;

    let _ = diskutil::unmount_disk(&bsd_path);
    diskutil::mount_disk(&bsd_path).context("mount after partition write")?;
    diskutil::unmount_disk_force(&bsd_path)
        .context("force-unmount before format (disk arbitration race)")?;

    diskutil::newfs_msdos_fat32(&partition_raw, &plan.label)
        .with_context(|| format!("formatting {partition_raw} as FAT32"))?;

    diskutil::mount_disk(&bsd_path).context("mount after format")?;
    let usb_mount = find_mount_for_label(&plan.label)
        .ok_or_else(|| anyhow!("formatted partition didn't appear in /Volumes"))?;

    stage_files(plan, &usb_mount).context("staging GRUB4DOS/FiraDisk XP payload")?;

    diskutil::unmount_disk(&bsd_path).context("unmount after staging files")?;
    diskutil::eject(&bsd_path).context("eject after write")?;

    println!(
        "\nusbwin: {} -> {} (Windows NT/XP FiraDisk mode) OK",
        plan.iso_path.display(),
        info.path
    );
    println!(
        "usbwin: boot entry 1 for text-mode setup; after reboot, boot USB again and choose entry 2"
    );
    Ok(())
}

fn validate_assets() -> Result<()> {
    if GRLDR_MBR.len() < 16 * SECTOR_SIZE as usize || GRLDR_MBR.len() % SECTOR_SIZE as usize != 0 {
        bail!(
            "embedded grldr.mbr has unexpected length {}",
            GRLDR_MBR.len()
        );
    }
    if GRLDR.is_empty() {
        bail!("embedded GRLDR asset is empty");
    }
    if FIRADISK_IMA.len() != 1_474_560 {
        bail!(
            "embedded FIRADISK.IMA has unexpected length {}; expected 1.44MB floppy image",
            FIRADISK_IMA.len()
        );
    }
    Ok(())
}

fn validate_capacity(plan: &WritePlan, info: &DeviceInfo) -> Result<()> {
    let needed = plan.iso_bytes
        + GRLDR.len() as u64
        + FIRADISK_IMA.len() as u64
        + MENU_LST.len() as u64
        + 16 * 1024 * 1024;
    let usable = info
        .size_bytes
        .saturating_sub(PARTITION_START_LBA as u64 * SECTOR_SIZE);
    if needed > usable {
        bail!(
            "NT/XP FiraDisk payload needs {} bytes; device has about {} usable bytes",
            needed,
            usable
        );
    }
    Ok(())
}

fn write_grub4dos_mbr_track(info: &DeviceInfo, verify: bool) -> Result<()> {
    let mut boot_track = GRLDR_MBR.to_vec();
    patch_partition_table(&mut boot_track, info.size_bytes)?;

    let mut dev = RawDevice::open(&info.path, OpenMode::ReadWrite, &info.model)
        .context("opening whole disk for GRUB4DOS MBR write")?;
    dev.write_at(0, &boot_track).map_err(anyhow_from_core)?;
    dev.sync().map_err(anyhow_from_core)?;

    if verify {
        let mut readback = vec![0u8; boot_track.len()];
        dev.read_at(0, &mut readback).map_err(anyhow_from_core)?;
        if readback != boot_track {
            bail!("GRUB4DOS MBR/boot-track write verify mismatch");
        }
    }
    Ok(())
}

fn patch_partition_table(mbr_track: &mut [u8], disk_size_bytes: u64) -> Result<()> {
    if mbr_track.len() < 512 {
        bail!("GRUB4DOS MBR asset shorter than one sector");
    }

    let disk_sectors = disk_size_bytes / SECTOR_SIZE;
    if disk_sectors <= PARTITION_START_LBA as u64 {
        bail!("device is too small for a partition starting at LBA {PARTITION_START_LBA}");
    }
    let partition_sectors = disk_sectors - PARTITION_START_LBA as u64;
    let partition_sectors_u32 = u32::try_from(partition_sectors)
        .context("device too large for this MBR-only prototype path (>2TiB partition)")?;

    let mut entry = [0u8; 16];
    entry[0] = 0x80; // active
    entry[1..4].copy_from_slice(&[0x20, 0x21, 0x00]); // CHS for LBA 2048: C=0,H=32,S=33
    entry[4] = 0x0c; // FAT32 LBA
    entry[5..8].copy_from_slice(&[0xfe, 0xff, 0xff]); // saturated end CHS
    entry[8..12].copy_from_slice(&PARTITION_START_LBA.to_le_bytes());
    entry[12..16].copy_from_slice(&partition_sectors_u32.to_le_bytes());

    mbr_track[446..462].copy_from_slice(&entry);
    mbr_track[462..510].fill(0);
    mbr_track[510..512].copy_from_slice(&[0x55, 0xaa]);
    Ok(())
}

fn stage_files(plan: &WritePlan, usb_mount: &Path) -> Result<()> {
    std::fs::write(usb_mount.join("GRLDR"), GRLDR)
        .with_context(|| format!("writing {}", usb_mount.join("GRLDR").display()))?;
    std::fs::write(usb_mount.join("menu.lst"), MENU_LST)
        .with_context(|| format!("writing {}", usb_mount.join("menu.lst").display()))?;
    std::fs::write(usb_mount.join("FIRADISK.IMA"), FIRADISK_IMA)
        .with_context(|| format!("writing {}", usb_mount.join("FIRADISK.IMA").display()))?;

    copy_iso_as_xp_iso(&plan.iso_path, &usb_mount.join("XP.ISO"))?;

    let _ = std::process::Command::new("sync").status();
    println!("usbwin: staged GRLDR, menu.lst, XP.ISO, FIRADISK.IMA");
    Ok(())
}

fn copy_iso_as_xp_iso(src: &Path, dest: &Path) -> Result<()> {
    const CHUNK: usize = 4 * 1024 * 1024;
    let total = std::fs::metadata(src)
        .with_context(|| format!("stat {}", src.display()))?
        .len();
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template("  {prefix:<6} {wide_bar:.cyan/blue} {msg}")?
            .progress_chars("█▓▒░ "),
    );
    pb.set_prefix("iso");
    pb.enable_steady_tick(Duration::from_millis(100));

    let mut input = File::open(src).with_context(|| format!("open {}", src.display()))?;
    let mut output = File::create(dest).with_context(|| format!("create {}", dest.display()))?;
    let mut buf = vec![0u8; CHUNK];
    let start = Instant::now();
    let mut copied = 0u64;
    loop {
        let n = input.read(&mut buf).context("read ISO")?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n]).context("write XP.ISO")?;
        copied += n as u64;
        pb.set_position(copied);
        update_iso_message(&pb, copied, total, start);
    }
    pb.finish();
    Ok(())
}

fn update_iso_message(pb: &ProgressBar, copied: u64, total: u64, start: Instant) {
    let elapsed = start.elapsed().as_secs_f64().max(0.01);
    let bps = copied as f64 / elapsed;
    let eta = if bps > 0.0 {
        (total - copied) as f64 / bps
    } else {
        0.0
    };
    pb.set_message(format!(
        "{:>10} / {:<10} @ {:>10}/s  ETA {}",
        human_bytes(copied),
        human_bytes(total),
        human_bytes(bps as u64),
        human_secs(eta as u64),
    ));
}

fn find_mount_for_label(label: &str) -> Option<PathBuf> {
    let target = PathBuf::from("/Volumes").join(label);
    for _ in 0..20 {
        if target.exists() {
            return Some(target);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn human_secs(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn anyhow_from_core(e: usbwin_core::Error) -> anyhow::Error {
    anyhow!(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_partition_table_matches_prototype_layout() {
        let mut track = vec![0u8; 8192];
        patch_partition_table(&mut track, 64_023_257_088).unwrap();

        let e = &track[446..462];
        assert_eq!(e[0], 0x80);
        assert_eq!(&e[1..4], &[0x20, 0x21, 0x00]);
        assert_eq!(e[4], 0x0c);
        assert_eq!(&e[5..8], &[0xfe, 0xff, 0xff]);
        assert_eq!(u32::from_le_bytes(e[8..12].try_into().unwrap()), 2048);
        assert_eq!(
            u32::from_le_bytes(e[12..16].try_into().unwrap()),
            125_043_376
        );
        assert_eq!(&track[510..512], &[0x55, 0xaa]);
    }

    #[test]
    fn menu_has_textmode_and_gui_continuation_entries() {
        assert!(MENU_LST.contains("title 1. XP text-mode setup"));
        assert!(MENU_LST.contains("title 2. Continue XP GUI-mode setup"));
        assert!(MENU_LST.contains("map --mem /XP.ISO (0xff)"));
        assert!(MENU_LST.contains("chainloader (hd0)+1"));
    }
}
