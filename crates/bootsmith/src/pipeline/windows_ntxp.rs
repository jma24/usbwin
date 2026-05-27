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

use bootsmith_core::{Config, Device, UnattendedConfig, WritePlan};
use bootsmith_disk::raw::{OpenMode, RawDevice};
use bootsmith_disk::DeviceInfo;

use super::diskutil;
use super::ntxp_floppy;
use super::ntxp_iso;
use super::ntxp_slipstream;
use super::ntxp_txtsetup;

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
# (fd1) duplicate disabled 2026-05-22 to test the Win2k 0x7B/0xC0000035
# name-collision hypothesis (FiraDisk loading twice). Restore if XP
# regresses or if Win2k still BSODs with the same code.
#map --mem /FIRADISK.IMA (fd1)
map --mem /XP.ISO (0xff)
map --hook
root (0xff)
chainloader (0xff)/I386/SETUPLDR.BIN

title 2. Continue XP GUI-mode setup from internal HDD
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /FIRADISK.IMA (fd0)
# (fd1) duplicate disabled 2026-05-22 to test the Win2k 0x7B/0xC0000035
# name-collision hypothesis (FiraDisk loading twice). Restore if XP
# regresses or if Win2k still BSODs with the same code.
#map --mem /FIRADISK.IMA (fd1)
map --mem /XP.ISO (0xff)
map --hook
root (hd0,0)
chainloader (hd0)+1

title 3. XP text-mode setup via ISO boot image
map (hd0) (hd1)
map (hd1) (hd0)
map --mem /FIRADISK.IMA (fd0)
# (fd1) duplicate disabled 2026-05-22 to test the Win2k 0x7B/0xC0000035
# name-collision hypothesis (FiraDisk loading twice). Restore if XP
# regresses or if Win2k still BSODs with the same code.
#map --mem /FIRADISK.IMA (fd1)
map --mem /XP.ISO (0xff)
map --hook
chainloader (0xff)
"#;

pub fn run(plan: &WritePlan, info: &DeviceInfo, config: &Config) -> Result<()> {
    let bsd_path = info.path.replace("/dev/r", "/dev/");
    let partition_raw = format!("{}s1", info.path);

    tracing::debug!(
        raw = %info.path,
        buffered = %bsd_path,
        partition_raw,
        ahci = config.ahci_driver_dir.is_some(),
        unattended = config.unattended.is_some(),
        "windows-ntxp pipeline begin"
    );

    validate_assets()?;
    validate_capacity(plan, info)?;

    tracing::debug!("step 1/7: unmount disk before GRUB4DOS MBR write");
    diskutil::unmount_disk(&bsd_path).context("unmount before GRUB4DOS MBR write")?;
    tracing::debug!("step 2/7: write GRUB4DOS MBR + boot track");
    write_grub4dos_mbr_track(info, config.verify).context("writing GRUB4DOS MBR/boot track")?;

    tracing::debug!("step 3/7: re-read partition table via unmount/mount cycle");
    if let Err(e) = diskutil::unmount_disk(&bsd_path) {
        tracing::debug!(error = %e, "first unmount errored (expected if nothing was mounted)");
    }
    diskutil::mount_disk(&bsd_path).context("mount after partition write")?;
    diskutil::unmount_disk_force(&bsd_path)
        .context("force-unmount before format (disk arbitration race)")?;

    tracing::debug!(partition = %partition_raw, label = %plan.label, "step 4/7: newfs_msdos FAT32");
    diskutil::newfs_msdos_fat32(&partition_raw, &plan.label)
        .with_context(|| format!("formatting {partition_raw} as FAT32"))?;

    tracing::debug!("step 5/7: mount formatted partition");
    diskutil::mount_disk(&bsd_path).context("mount after format")?;
    let usb_mount = find_mount_for_label(&plan.label)
        .ok_or_else(|| anyhow!("formatted partition didn't appear in /Volumes"))?;
    tracing::debug!(mount = %usb_mount.display(), "USB mount resolved");

    tracing::debug!("step 6/7: stage GRLDR / menu.lst / XP.ISO / FIRADISK.IMA");
    stage_files(plan, &usb_mount, config).context("staging GRUB4DOS/FiraDisk XP payload")?;

    tracing::debug!("step 7/7: unmount + eject");
    diskutil::unmount_disk(&bsd_path).context("unmount after staging files")?;
    diskutil::eject(&bsd_path).context("eject after write")?;

    println!(
        "\nbootsmith: {} -> {} (Windows NT/XP FiraDisk mode) OK",
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

fn stage_files(plan: &WritePlan, usb_mount: &Path, config: &Config) -> Result<()> {
    tracing::debug!(size = GRLDR.len(), "write GRLDR");
    std::fs::write(usb_mount.join("GRLDR"), GRLDR)
        .with_context(|| format!("writing {}", usb_mount.join("GRLDR").display()))?;
    tracing::debug!(size = MENU_LST.len(), "write menu.lst");
    std::fs::write(usb_mount.join("menu.lst"), MENU_LST)
        .with_context(|| format!("writing {}", usb_mount.join("menu.lst").display()))?;
    let xp_iso = usb_mount.join("XP.ISO");
    tracing::debug!(src = %plan.iso_path.display(), dest = %xp_iso.display(), "copy ISO → XP.ISO");
    copy_iso_as_xp_iso(&plan.iso_path, &xp_iso)?;

    // FiraDisk floppy is the embedded image verbatim. The previous
    // approach (merge user's --ahci-driver-dir TXTSETUP.OEM into the
    // floppy, add the .sys/.inf/.cat files, generate a winnt.sif with
    // [MassStorageDrivers] + OemPreinstall=Yes) hit XP setup error 18
    // at oemdisk.c:1747 on real hardware. Research confirmed
    // OemPreinstall=Yes requires $OEM$\Textmode\ on the install source
    // (which the RAM-mapped ISO doesn't have) and that on-floppy
    // [MassStorageDrivers] only auto-loads the [Defaults] entry anyway.
    // The canonical XP-on-AHCI solution is slipstreaming iaStor into
    // I386 of the install source itself -- handled below for the
    // staged XP.ISO. The floppy stays single-purpose: FiraDisk only.
    let mut firadisk = FIRADISK_IMA.to_vec();

    if let Some(ahci_dir) = &config.ahci_driver_dir {
        slipstream_ahci_into_iso(&xp_iso, ahci_dir)
            .with_context(|| format!("slipstreaming AHCI drivers from {}", ahci_dir.display()))?;
    }

    // winnt.sif is ONLY generated when --unattended is set. An earlier
    // attempt to auto-resolve the GUI-mode "browse to F:\i386\iaStor.sys"
    // prompt by injecting a minimal sif with OemPnPDriversPath="i386"
    // (even in interactive mode) caused a separate regression: GUI-mode
    // setup started prompting for the `asms` side-by-side assembly
    // folder mid-file-copy. The OemPnPDriversPath path semantics are
    // looser than the docs imply and clearly interact with the
    // file-copy logic in ways we don't fully understand. Until we have
    // a verified-clean fix, the slipstream auto-loads iaStor in
    // text-mode and the user clicks through one "browse" prompt at
    // GUI-mode -- acceptable trade-off vs. breaking the install.
    if let Some(unattended) = &config.unattended {
        let sif = generate_winnt_sif(unattended);
        ntxp_floppy::add_winnt_sif(&mut firadisk, sif.as_bytes())
            .context("injecting A:\\WINNT.SIF into FiraDisk floppy image")?;
        ntxp_iso::inject_winnt_sif(&xp_iso, sif.as_bytes())
            .with_context(|| format!("injecting I386/WINNT.SIF into {}", xp_iso.display()))?;
        println!("bootsmith: injected A:\\WINNT.SIF and I386/WINNT.SIF for unattended setup");
    }

    tracing::debug!(size = firadisk.len(), "write FIRADISK.IMA");
    std::fs::write(usb_mount.join("FIRADISK.IMA"), &firadisk)
        .with_context(|| format!("writing {}", usb_mount.join("FIRADISK.IMA").display()))?;

    let _ = std::process::Command::new("sync").status();
    println!("bootsmith: staged GRLDR, menu.lst, XP.ISO, FIRADISK.IMA");
    Ok(())
}

/// Slipstream a user-supplied F6 driver pack (e.g. Intel iaStor for AHCI)
/// into the staged XP.ISO's `I386` directory and patch
/// `I386\TXTSETUP.SIF` so XP text-mode setup treats the driver as inbox.
/// This is the only auto-load mechanism that works for our RAM-mapped
/// ISO + FiraDisk pipeline -- see the long comment in `stage_files`.
///
/// Steps:
/// 1. Parse the user's TXTSETUP.OEM. Get the list of files to copy and
///    the per-controller [HardwareIds.scsi.X] PCI ids.
/// 2. Append each driver file (.sys, .inf, .cat) into I386 of the ISO.
/// 3. Read I386/TXTSETUP.SIF, patch the four sections, write back via
///    replace_file_in_i386 (it relocates since the file grows).
fn slipstream_ahci_into_iso(xp_iso: &Path, ahci_dir: &Path) -> Result<()> {
    if !ahci_dir.is_dir() {
        bail!(
            "--ahci-driver-dir {} is not a directory",
            ahci_dir.display()
        );
    }
    let oem_path = find_case_insensitive(ahci_dir, "txtsetup.oem")?
        .ok_or_else(|| anyhow!("{} has no TXTSETUP.OEM", ahci_dir.display()))?;
    let user_oem = std::fs::read_to_string(&oem_path)
        .with_context(|| format!("read {}", oem_path.display()))?;

    let referenced = ntxp_txtsetup::referenced_filenames(&user_oem)
        .context("parse file list from user TXTSETUP.OEM")?;
    if referenced.is_empty() {
        bail!(
            "user TXTSETUP.OEM at {} declares no driver files",
            oem_path.display()
        );
    }
    let hardware_ids = ntxp_txtsetup::hardware_ids(&user_oem)
        .context("parse hardware IDs from user TXTSETUP.OEM")?;

    // Resolve the storage service name from the user's [Files.scsi.X]
    // driver= lines. Every controller in a typical Intel pack maps to
    // the same service ("iaStor"); if they diverge we'd need multiple
    // slipstream passes, which we don't currently support.
    let services = collect_services(&user_oem)?;
    if services.len() != 1 {
        bail!(
            "user TXTSETUP.OEM at {} declares multiple driver services ({:?}); \
             slipstream currently supports one service per pack",
            oem_path.display(),
            services
        );
    }
    let service = services.into_iter().next().unwrap();

    // Display name: take it from the FIRST [scsi] entry whose
    // [Files.scsi.X] uses this service. Falls back to the service name.
    let display_name = ntxp_txtsetup::scsi_controllers(&user_oem)
        .context("parse [scsi] controllers from user TXTSETUP.OEM")?
        .into_iter()
        .find(|c| {
            c.driver
                .as_deref()
                .map(|f| {
                    f.to_ascii_lowercase()
                        .ends_with(&format!("{}.sys", service.to_ascii_lowercase()))
                })
                .unwrap_or(false)
        })
        .map(|c| c.display_name)
        .unwrap_or_else(|| service.clone());

    // 1. Append every referenced file into I386. macOS buffers the ISO
    // copy from copy_iso_as_xp_iso aggressively, so the first sync_all
    // inside append_file_to_i386 forces hundreds of MB of buffered
    // data to flush -- visibly slow without a status line, since the
    // iso progress bar already showed 100%.
    //
    // Attempt 2026-05-26: also duplicated each .sys at the ISO root so
    // GUI-mode PnP's "Files Needed" dialog (which defaults to F:\)
    // would auto-find iaStor.sys. Caused the same asms-folder prompt
    // regression that OemPnPDriversPath had earlier. Any modification
    // that touches the ISO root directory layout in a way XP doesn't
    // expect seems to disturb GUI-mode setup's side-by-side assembly
    // copy phase. Reverted; one click at GUI-mode is the 1.0 trade.
    let total_steps = referenced.len() + 2;
    let mut step = 1usize;
    for name in &referenced {
        let path = find_case_insensitive(ahci_dir, name)?
            .ok_or_else(|| anyhow!("{} is missing {}", ahci_dir.display(), name))?;
        let contents = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let iso_name = iso_long_name(name);
        println!(
            "  slipstream [{step}/{total_steps}] inject I386/{} ({:.0} KiB)",
            iso_name,
            contents.len() as f64 / 1024.0,
        );
        ntxp_iso::append_file_to_i386(xp_iso, iso_name.as_bytes(), &contents)
            .with_context(|| format!("append {} into I386 of {}", name, xp_iso.display()))?;
        step += 1;
    }

    // 2. Read existing TXTSETUP.SIF, patch, replace. XP SP3 master ISO
    // PVD records use bare names (no `;1` version suffix); iso_name_eq
    // accepts either form on the expected side.
    println!("  slipstream [{step}/{total_steps}] patch I386/TXTSETUP.SIF");
    step += 1;
    let existing_sif = ntxp_iso::read_file_from_i386(xp_iso, b"TXTSETUP.SIF")
        .context("read existing I386/TXTSETUP.SIF from staged XP.ISO")?
        .ok_or_else(|| anyhow!("staged XP.ISO has no I386/TXTSETUP.SIF"))?;
    let existing_sif_text = std::str::from_utf8(&existing_sif)
        .context("I386/TXTSETUP.SIF is not valid ASCII/UTF-8")?;
    let patched = ntxp_slipstream::patch_txtsetup_sif(
        existing_sif_text,
        &service,
        &display_name,
        &referenced,
        &hardware_ids,
    )
    .context("patch I386/TXTSETUP.SIF for AHCI slipstream")?;
    ntxp_iso::replace_file_in_i386(xp_iso, b"TXTSETUP.SIF", patched.text.as_bytes())
        .context("write patched I386/TXTSETUP.SIF back to staged XP.ISO")?;

    // 3. Patch DOSNET.INF [Files] manifest. TXTSETUP.SIF alone tells
    // text-mode setup ABOUT the driver (and ramdrive-copies it from
    // [SourceDisksFiles]), but the regular file-copy phase consults
    // DOSNET.INF to decide what's available on the install source.
    // Without the d1,iastor.sys entries setup errors with "The file
    // iaStor.sys could not be found" mid-text-mode.
    println!("  slipstream [{step}/{total_steps}] patch I386/DOSNET.INF");
    let existing_dosnet = ntxp_iso::read_file_from_i386(xp_iso, b"DOSNET.INF")
        .context("read existing I386/DOSNET.INF from staged XP.ISO")?
        .ok_or_else(|| anyhow!("staged XP.ISO has no I386/DOSNET.INF"))?;
    let existing_dosnet_text = std::str::from_utf8(&existing_dosnet)
        .context("I386/DOSNET.INF is not valid ASCII/UTF-8")?;
    let patched_dosnet = ntxp_slipstream::patch_dosnet_inf(existing_dosnet_text, &referenced)
        .context("patch I386/DOSNET.INF for AHCI slipstream")?;
    ntxp_iso::replace_file_in_i386(xp_iso, b"DOSNET.INF", patched_dosnet.as_bytes())
        .context("write patched I386/DOSNET.INF back to staged XP.ISO")?;

    println!(
        "bootsmith: slipstreamed AHCI driver \"{}\" ({}) into XP.ISO: \
         {} files + {} PCI hardware IDs (TXTSETUP.SIF + DOSNET.INF patched)",
        patched.additions.display_name,
        patched.additions.service,
        patched.additions.files.len(),
        patched.additions.hardware_id_count,
    );
    Ok(())
}

/// Distinct service names declared by the `[Files.scsi.X]` `driver=`
/// lines of a txtsetup.oem. The third comma-separated field of each
/// `driver = diskN, file.sys, ServiceName` line is the service.
fn collect_services(oem: &str) -> Result<Vec<String>> {
    let controllers = ntxp_txtsetup::scsi_controllers(oem)
        .context("parse [scsi] controllers from user TXTSETUP.OEM")?;
    // We need the service name from the raw `driver = diskN, file.sys,
    // ServiceName` line, which scsi_controllers doesn't currently
    // expose. Re-walk [Files.scsi.X] sections here for it.
    let mut services: Vec<String> = Vec::new();
    for c in &controllers {
        let section_header = format!("[Files.scsi.{}]", c.id);
        let Some(start) = oem.find(&section_header) else {
            continue;
        };
        let body_start = start + section_header.len();
        let body_end = oem[body_start..]
            .find("\n[")
            .map(|n| body_start + n)
            .unwrap_or(oem.len());
        for line in oem[body_start..body_end].lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed
                .strip_prefix("driver")
                .map(|r| r.trim_start())
                .filter(|r| r.starts_with('='))
            {
                let rhs = rest[1..].trim();
                let mut fields = rhs.split(',');
                let _disk = fields.next();
                let _file = fields.next();
                if let Some(svc) = fields.next() {
                    let svc = svc.trim().to_string();
                    if !svc.is_empty() && !services.iter().any(|s| s.eq_ignore_ascii_case(&svc)) {
                        services.push(svc);
                    }
                }
            }
        }
    }
    if services.is_empty() {
        bail!("no driver service names found in [Files.scsi.X] sections");
    }
    Ok(services)
}

/// Convert a long-form filename to the ISO9660 record name used inside
/// `I386`: uppercase, no version suffix (XP SP3 masters use bare names).
fn iso_long_name(name: &str) -> String {
    name.to_ascii_uppercase()
}

fn find_case_insensitive(dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("readdir {}", dir.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", dir.display()))?;
        if entry.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
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

fn generate_winnt_sif(config: &UnattendedConfig) -> String {
    let mut out = String::new();
    out.push_str("[Data]\r\n");
    out.push_str("AutoPartition=0\r\n");
    out.push_str("MsDosInitiated=\"0\"\r\n");
    out.push_str("UnattendedInstall=\"Yes\"\r\n");
    out.push_str("\r\n[Unattended]\r\n");
    out.push_str("UnattendMode=DefaultHide\r\n");
    out.push_str("OemSkipEula=Yes\r\n");
    out.push_str("TargetPath=\\WINDOWS\r\n");
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

fn anyhow_from_core(e: bootsmith_core::Error) -> anyhow::Error {
    anyhow::Error::new(e)
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

    #[test]
    fn generated_winnt_sif_keeps_manual_partitioning() {
        let sif = generate_winnt_sif(&UnattendedConfig {
            product_key: Some("AAAAA-BBBBB-CCCCC-DDDDD-EEEEE".into()),
            full_name: "QA User".into(),
            organization: "bootsmith".into(),
            computer_name: "*".into(),
            admin_password: None,
            timezone: Some(35),
        });

        assert!(sif.contains("[Data]\r\n"));
        assert!(sif.contains("AutoPartition=0\r\n"));
        assert!(sif.contains("UnattendedInstall=\"Yes\"\r\n"));
        assert!(sif.contains("OemSkipEula=Yes\r\n"));
        assert!(sif.contains("Repartition=No\r\n"));
        assert!(sif.contains("FileSystem=*\r\n"));
        assert!(sif.contains("AdminPassword=*\r\n"));
        assert!(sif.contains("TimeZone=35\r\n"));
        assert!(sif.contains("ProductKey=AAAAA-BBBBB-CCCCC-DDDDD-EEEEE\r\n"));
        assert!(sif.contains("FullName=\"QA User\"\r\n"));
        assert!(sif.contains("ComputerName=*\r\n"));
        assert!(!sif.contains("DestinationDiskNumber"));
        assert!(!sif.contains("DestinationPartitionNumber"));
        assert!(!sif.contains("OemPreinstall"));
        // OemPnPDriversPath="i386" auto-resolved the GUI-mode iaStor
        // browse prompt but caused a worse regression: GUI-mode started
        // prompting for the `asms` side-by-side assembly folder during
        // file copy. Keep it out until we understand the asms
        // interaction.
        assert!(!sif.contains("OemPnPDriversPath"));
    }

    #[test]
    fn generated_winnt_sif_quotes_user_values() {
        let sif = generate_winnt_sif(&UnattendedConfig {
            product_key: None,
            full_name: "A \"Quoted\" User".into(),
            organization: "C:\\Lab".into(),
            computer_name: "XPTEST".into(),
            admin_password: Some("p@ss \"word\"".into()),
            timezone: None,
        });

        assert!(sif.contains("AdminPassword=\"p@ss \\\"word\\\"\"\r\n"));
        assert!(sif.contains("FullName=\"A \\\"Quoted\\\" User\"\r\n"));
        assert!(sif.contains("OrgName=\"C:\\\\Lab\"\r\n"));
        assert!(sif.contains("ComputerName=\"XPTEST\"\r\n"));
    }
}

