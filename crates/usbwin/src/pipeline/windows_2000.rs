//! Windows 2000 (NT 5.0) install pipeline.
//!
//! Mirrors `windows_ntxp` (XP/2003 + FiraDisk) but swaps FiraDisk for
//! SVBus, because FiraDisk's SCSI miniport collides with the NT 5.0
//! storage stack (hardware-confirmed 2026-05-22:
//! `STOP 0x0000007B 0xF6063848 0xC0000034`). The GRUB4DOS RAM-mapped-ISO
//! chain shape is otherwise identical: map the install ISO into RAM as
//! `(0xff)`, map the SVBus F6 floppy as `(fd0)`, drive-swap, chainload
//! `SETUPLDR.BIN`. See `docs/WIN2K_SVBUS.md`.
//!
//! Shares the `ntxp_floppy::add_winnt_sif` and `ntxp_iso::inject_winnt_sif`
//! helpers with the XP path; those are generic FAT12 / ISO9660
//! manipulators and don't care which driver is on the floppy.

use anyhow::{anyhow, bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use usbwin_core::{Config, Device, UnattendedConfig, WritePlan};
use usbwin_disk::raw::{OpenMode, RawDevice};
use usbwin_disk::DeviceInfo;

use super::diskutil;
use super::ntxp_floppy;
use super::ntxp_iso;

const SECTOR_SIZE: u64 = 512;
const PARTITION_START_LBA: u32 = 2048;

/// GRUB4DOS 0.4.5c (2015-05-18) — the SVBus-compatible grldr per upstream
/// `ReadMe.txt`. We do NOT reuse the XP path's 0.4.6a 2020-08-09 grldr:
/// chenall/grub4dos issue #154 documents a low-memory regression
/// introduced 2017-02-04 that breaks SVBus's `$INT13SFGRUB4DOS` signature
/// scan (and broke XP+NTLDR RAM-loaded ISO boot at the same time).
/// Hardware-confirmed on the Dell E6410 2026-05-22: 0.4.6a 2020-08-09 +
/// SVBus = `STOP 0x7B 0xC0000034` (zero drives enumerated). See
/// `win2k_assets/PROVENANCE.md`.
const GRLDR: &[u8] = include_bytes!("win2k_assets/grldr");
const GRLDR_MBR: &[u8] = include_bytes!("ntxp_assets/grldr.mbr");
const SVBUS_IMA: &[u8] = include_bytes!("win2k_assets/svbus.ima");

const MENU_LST: &str = r#"timeout 10
default 0

title 1. Win2k text-mode setup from RAM ISO (SVBus)
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /SVBUS.IMA (fd0)
map --mem /WIN2K.ISO (0xff)
map --hook
root (0xff)
chainloader (0xff)/I386/SETUPLDR.BIN

title 2. Continue Win2k GUI-mode setup from internal HDD
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /SVBUS.IMA (fd0)
map --mem /WIN2K.ISO (0xff)
map --hook
root (hd0,0)
chainloader (hd0)+1

title 3. Win2k text-mode setup via ISO boot image
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /SVBUS.IMA (fd0)
map --mem /WIN2K.ISO (0xff)
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

    stage_files(plan, &usb_mount, config).context("staging GRUB4DOS/SVBus Win2k payload")?;

    diskutil::unmount_disk(&bsd_path).context("unmount after staging files")?;
    diskutil::eject(&bsd_path).context("eject after write")?;

    println!(
        "\nusbwin: {} -> {} (Windows 2000 SVBus mode) OK",
        plan.iso_path.display(),
        info.path
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
    if SVBUS_IMA.len() != 1_474_560 {
        bail!(
            "embedded SVBUS.IMA has unexpected length {}; expected 1.44MB floppy image",
            SVBUS_IMA.len()
        );
    }
    Ok(())
}

fn validate_capacity(plan: &WritePlan, info: &DeviceInfo) -> Result<()> {
    let needed = plan.iso_bytes
        + GRLDR.len() as u64
        + SVBUS_IMA.len() as u64
        + MENU_LST.len() as u64
        + 16 * 1024 * 1024;
    let usable = info
        .size_bytes
        .saturating_sub(PARTITION_START_LBA as u64 * SECTOR_SIZE);
    if needed > usable {
        bail!(
            "Win2k SVBus payload needs {} bytes; device has about {} usable bytes",
            needed,
            usable
        );
    }
    Ok(())
}

fn write_grub4dos_mbr_track(info: &DeviceInfo, verify: bool) -> Result<()> {
    let mut dev = RawDevice::open(&info.path, OpenMode::ReadWrite, &info.model)
        .context("opening whole disk for GRUB4DOS MBR write")?;
    let mut boot_track = GRLDR_MBR.to_vec();
    patch_partition_table(&mut boot_track, dev.size_bytes().map_err(anyhow_from_core)?)?;

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
    entry[0] = 0x80;
    entry[1..4].copy_from_slice(&[0x20, 0x21, 0x00]);
    entry[4] = 0x0c;
    entry[5..8].copy_from_slice(&[0xfe, 0xff, 0xff]);
    entry[8..12].copy_from_slice(&PARTITION_START_LBA.to_le_bytes());
    entry[12..16].copy_from_slice(&partition_sectors_u32.to_le_bytes());

    mbr_track[446..462].copy_from_slice(&entry);
    mbr_track[462..510].fill(0);
    mbr_track[510..512].copy_from_slice(&[0x55, 0xaa]);
    Ok(())
}

fn stage_files(plan: &WritePlan, usb_mount: &Path, config: &Config) -> Result<()> {
    std::fs::write(usb_mount.join("GRLDR"), GRLDR)
        .with_context(|| format!("writing {}", usb_mount.join("GRLDR").display()))?;
    std::fs::write(usb_mount.join("menu.lst"), MENU_LST)
        .with_context(|| format!("writing {}", usb_mount.join("menu.lst").display()))?;
    let win2k_iso = usb_mount.join("WIN2K.ISO");
    copy_iso_to(&plan.iso_path, &win2k_iso)?;
    if let Some(unattended) = &config.unattended {
        let sif = generate_winnt_sif(unattended);
        let mut svbus = SVBUS_IMA.to_vec();
        ntxp_floppy::add_winnt_sif(&mut svbus, sif.as_bytes())
            .context("injecting A:\\WINNT.SIF into SVBus floppy image")?;
        std::fs::write(usb_mount.join("SVBUS.IMA"), &svbus)
            .with_context(|| format!("writing {}", usb_mount.join("SVBUS.IMA").display()))?;
        println!("usbwin: injected A:\\WINNT.SIF into staged SVBUS.IMA");

        ntxp_iso::inject_winnt_sif(&win2k_iso, sif.as_bytes())
            .with_context(|| format!("injecting I386/WINNT.SIF into {}", win2k_iso.display()))?;
        println!("usbwin: injected I386/WINNT.SIF into staged WIN2K.ISO for unattended setup");
    } else {
        std::fs::write(usb_mount.join("SVBUS.IMA"), SVBUS_IMA)
            .with_context(|| format!("writing {}", usb_mount.join("SVBUS.IMA").display()))?;
    }

    let _ = std::process::Command::new("sync").status();
    println!("usbwin: staged GRLDR, menu.lst, WIN2K.ISO, SVBUS.IMA");
    Ok(())
}

fn copy_iso_to(src: &Path, dst: &Path) -> Result<()> {
    let mut input = File::open(src).with_context(|| format!("opening {}", src.display()))?;
    let mut output =
        File::create(dst).with_context(|| format!("creating {}", dst.display()))?;
    let total = input
        .metadata()
        .map(|m| m.len())
        .with_context(|| format!("stat {}", src.display()))?;
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template("{spinner} {msg}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );

    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let start = Instant::now();
    let mut copied = 0u64;
    loop {
        let n = input.read(&mut buf).context("read ISO")?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n]).context("write WIN2K.ISO")?;
        copied += n as u64;
        pb.set_position(copied);
        update_iso_message(&pb, copied, total, start);
    }
    pb.finish();
    Ok(())
}

fn generate_winnt_sif(config: &UnattendedConfig) -> String {
    let mut out = String::new();
    out.push_str("[Data]\r\n");
    out.push_str("AutoPartition=0\r\n");
    out.push_str("MsDosInitiated=\"0\"\r\n");
    out.push_str("UnattendedInstall=\"Yes\"\r\n");
    out.push_str("\r\n[Unattended]\r\n");
    out.push_str("UnattendMode=DefaultHide\r\n");
    out.push_str("OemSkipEula=Yes\r\n");
    out.push_str("TargetPath=\\WINNT\r\n");
    out.push_str("Repartition=No\r\n");
    out.push_str("FileSystem=*\r\n");
    out.push_str("DriverSigningPolicy=Ignore\r\n");
    out.push_str("NonDriverSigningPolicy=Ignore\r\n");
    out.push_str("\r\n[GuiUnattended]\r\n");
    match &config.admin_password {
        Some(password) => {
            out.push_str("AdminPassword=");
            out.push_str(&quoted_sif_value(password));
            out.push_str("\r\n");
        }
        None => out.push_str("AdminPassword=*\r\n"),
    }
    out.push_str("EncryptedAdminPassword=NO\r\n");
    out.push_str("OEMSkipRegional=1\r\n");
    out.push_str("OemSkipWelcome=1\r\n");
    if let Some(timezone) = config.timezone {
        out.push_str(&format!("TimeZone={timezone}\r\n"));
    }
    out.push_str("\r\n[UserData]\r\n");
    if let Some(product_key) = &config.product_key {
        out.push_str("ProductKey=");
        out.push_str(product_key);
        out.push_str("\r\n");
    }
    out.push_str("FullName=");
    out.push_str(&quoted_sif_value(&config.full_name));
    out.push_str("\r\nOrgName=");
    out.push_str(&quoted_sif_value(&config.organization));
    out.push_str("\r\nComputerName=");
    if config.computer_name == "*" {
        out.push('*');
    } else {
        out.push_str(&quoted_sif_value(&config.computer_name));
    }
    out.push_str("\r\n\r\n[Identification]\r\n");
    out.push_str("JoinWorkgroup=WORKGROUP\r\n");
    out.push_str("\r\n[Networking]\r\n");
    out.push_str("InstallDefaultComponents=Yes\r\n");
    out
}

fn quoted_sif_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
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
    fn svbus_ima_has_floppy_size() {
        assert_eq!(SVBUS_IMA.len(), 1_474_560);
    }

    #[test]
    fn menu_references_svbus_and_win2k_iso() {
        assert!(MENU_LST.contains("map --mem /SVBUS.IMA (fd0)"));
        assert!(MENU_LST.contains("map --mem /WIN2K.ISO (0xff)"));
        assert!(MENU_LST.contains("title 1. Win2k text-mode setup"));
        assert!(MENU_LST.contains("title 2. Continue Win2k GUI-mode setup"));
        assert!(!MENU_LST.contains("FIRADISK"));
        assert!(!MENU_LST.contains("XP.ISO"));
    }

    #[test]
    fn generated_winnt_sif_targets_winnt_directory() {
        let sif = generate_winnt_sif(&UnattendedConfig {
            product_key: Some("AAAAA-BBBBB-CCCCC-DDDDD-EEEEE".into()),
            full_name: "QA User".into(),
            organization: "usbwin".into(),
            computer_name: "*".into(),
            admin_password: None,
            timezone: Some(35),
        });
        assert!(sif.contains("TargetPath=\\WINNT"));
        assert!(sif.contains("AutoPartition=0"));
        assert!(sif.contains("ProductKey=AAAAA-BBBBB-CCCCC-DDDDD-EEEEE"));
    }
}
