//! Windows XP install USB pipeline (v0.3).
//!
//! Differences vs Win 7+ mode (see windows.rs):
//!   - Boot records: `ms-sys --mbr` (Win 2000/XP/2003 MBR) + `--fat32nt`
//!     (NTLDR-loading FAT32 PBR), instead of `--mbr7` / `--fat32pe`.
//!   - Post-copy SIF modification: edit I386/TXTSETUP.SIF on the USB to
//!     move USB drivers into [BootBusExtenders.Load] so XP text-mode
//!     setup recognizes the USB as boot media (the WinSetupFromUSB recipe).
//!   - Default label "WINXP" (vs "WIN7").
//!
//! What this does NOT YET do (planned chunks 6-7 per docs/V0.3_WINDOWS_XP.md):
//!   - WaitBT/Wait4UFD driver injection (`--xp-waiters /path/`)
//!   - winnt.sif answer file generation (`--xp-unattended`)
//! Without those, XP install will reach the text-mode setup phase and
//! complete the first stage. The graphical setup phase may hit a 0x7B
//! BSOD on hardware that initializes USB late; if you see that, we'll
//! land chunk 6.

use anyhow::{anyhow, bail, Context, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use usbwin_core::{BootRecordImpl, Config, Device, WritePlan};
use usbwin_disk::raw::{OpenMode, RawDevice};
use usbwin_disk::DeviceInfo;

use super::boot_records;
use super::diskutil;
use super::windows_xp_sif;
use super::windows_xp_unattended;
use super::xp_staging;

pub fn run(plan: &WritePlan, info: &DeviceInfo, config: &Config) -> Result<()> {
    let bsd_path = info.path.replace("/dev/r", "/dev/");
    let partition_raw = format!("{}s1", info.path);
    let partition_bsd = format!("{bsd_path}s1");

    // Boot records are mandatory for XP; resolve the selected backend up
    // front so we fail before any destructive write.
    let ms_sys = match config.boot_record_impl {
        BootRecordImpl::MsSys => Some(diskutil::find_ms_sys()?),
        BootRecordImpl::Bootrec => {
            boot_records::ensure_embedded_blobs()?;
            None
        }
    };

    // Validate optional inputs up front so we don't get halfway through a
    // write and discover a missing file.
    if let Some(dir) = &config.xp_waiters_dir {
        for f in &["WaitBT.sys", "Wait4UFD.sys"] {
            let p = dir.join(f);
            if !p.exists() {
                bail!(
                    "--xp-waiters directory {} is missing {f}. Both WaitBT.sys and \
                     Wait4UFD.sys must be present.",
                    dir.display()
                );
            }
        }
    }

    diskutil::unmount_disk(&bsd_path).context("unmount before partition write")?;
    write_mbr_sector(info)?;
    let _ = diskutil::unmount_disk(&bsd_path);
    diskutil::mount_disk(&bsd_path).context("mount after partition write")?;
    diskutil::unmount_disk_force(&bsd_path)
        .context("force-unmount before format (disk arbitration race)")?;

    diskutil::newfs_msdos_fat32(&partition_raw, &plan.label)
        .with_context(|| format!("formatting {partition_raw} as FAT32"))?;

    diskutil::mount_disk(&bsd_path).context("mount after format")?;
    let usb_mount = find_mount_for_label(&plan.label)
        .ok_or_else(|| anyhow!("formatted partition didn't appear in /Volumes"))?;

    let iso_mount = diskutil::hdiutil_attach_iso(&plan.iso_path)
        .with_context(|| format!("attaching ISO {}", plan.iso_path.display()))?;

    let copy_result = copy_iso_contents(&iso_mount, &usb_mount);
    let _ = diskutil::hdiutil_detach(&iso_mount);
    copy_result.context("copying XP ISO contents to USB")?;

    // Apply the WinSetupFromUSB txtsetup.sif modification on the copied file.
    apply_txtsetup_mods(&usb_mount, config.xp_waiters_dir.as_deref())
        .context("modifying TXTSETUP.SIF on USB")?;

    // Optional: write the unattended answer file. Goes at the root for
    // USB-bootstrapped setup (and also at I386\ for source compatibility).
    if config.xp_unattended {
        write_unattended(&usb_mount, config).context("writing winnt.sif")?;
    }

    // Stage \NTLDR, \NTDETECT.COM, \$LDR$, \boot.ini at the partition root.
    // Without these, the PBR loads \NTLDR (which isn't there yet — XP ISOs
    // keep it in \I386\) and boots fail before NTLDR even runs. WinSetupFromUSB
    // recipe; see pipeline/xp_staging.rs.
    let i386 = xp_staging::find_i386_dir(&usb_mount)?;
    xp_staging::stage_root_boot_files(&usb_mount, &i386)
        .context("staging XP boot files at root")?;
    println!("usbwin: staged NTLDR, NTDETECT.COM, $LDR$, boot.ini at USB root");

    diskutil::unmount_disk(&bsd_path).context("unmount before boot records")?;

    // Write the XP-era boot records. Both backends use bootrec's MBR_XP
    // (already in sector 0 from write_mbr_sector); only the PBR backend
    // varies. ms-sys's `--mbr` is unused: its XP-era boot code at offset
    // 0x9b-0xa3 loads DL from [bp+0] (= the active flag, 0x80), hardcoding
    // drive 0x80 instead of preserving the BIOS-supplied DL. On hardware
    // where the BIOS doesn't enumerate the USB stick as drive 0x80, the
    // ms-sys XP MBR reads the wrong drive and reports "Missing operating
    // system" (verified on Dell E6410, 2026-05-19, byte dumps in
    // /tmp/xp_mssys_*.hex from that session). bootrec's MBR_XP saves DL
    // before any processing and works on the same hardware. The MBR's
    // job is OS-agnostic (chainload the active partition's PBR), so
    // using bootrec's MBR for the ms-sys PBR path is correct.
    diskutil::unmount_disk_force(&bsd_path)
        .context("force-unmount before PBR write")?;
    match config.boot_record_impl {
        BootRecordImpl::MsSys => {
            let ms_sys = ms_sys.as_ref().expect("ms-sys resolved above");
            diskutil::ms_sys_fat32nt(ms_sys, &partition_bsd)
                .context("ms-sys --fat32nt (writing NTLDR-loading FAT32 PBR)")?;
        }
        BootRecordImpl::Bootrec => {
            splice_ntldr_pbr(&partition_raw, &info.model, config.verify)
                .context("bootrec FAT32 NTLDR PBR splice")?;
        }
    }

    // Generate \$WIN_NT$.~BT\BOOTSECT.DAT: read back the on-disk PBR,
    // replace the 11-byte "NTLDR      " filename with "$LDR$      ", write
    // as a file. NTLDR loads it as a bootsector entry via boot.ini, which
    // then chainloads $LDR$ (setupldr.bin) to start text-mode setup.
    //
    // Non-fatal: bootrec's NTLDR multi-sector PBR currently puts the NTLDR
    // string in stage 2 (sector 2), unreachable from a single-sector
    // BOOTSECT.DAT load. We skip with a warning rather than aborting so
    // the user can still test the chain up to the "<Windows root>\\system32
    // \\hal.dll missing" fallback NTLDR shows when BOOTSECT.DAT is absent.
    // ms-sys's --fat32nt puts the string at offset 0x170 in sector 0, so
    // this works for `--boot-record=ms-sys`. The proper fix (a bootrec
    // primitive that emits a single-sector raw-LBA loader for $LDR$) is
    // tracked separately.
    let pbr_bytes = read_pbr_sector0(&partition_raw, &info.model)
        .context("reading PBR back for BOOTSECT.DAT generation")?;
    let bootsect_dat = xp_staging::build_bootsect_dat(&pbr_bytes);

    // Re-mount to write the file (or to leave it absent), then unmount.
    diskutil::mount_disk(&bsd_path)
        .context("re-mount to write BOOTSECT.DAT")?;
    let usb_mount2 = find_mount_for_label(&plan.label).ok_or_else(|| {
        anyhow!("re-mounted partition didn't reappear in /Volumes")
    })?;
    match bootsect_dat {
        Ok(bytes) => {
            xp_staging::write_bootsect_dat(&usb_mount2, &bytes)
                .context("writing $WIN_NT$.~BT/BOOTSECT.DAT")?;
            println!(
                "usbwin: wrote {}/$WIN_NT$.~BT/BOOTSECT.DAT ({} bytes, patched from on-disk PBR)",
                usb_mount2.display(),
                bytes.len()
            );
        }
        Err(e) => {
            eprintln!();
            eprintln!("usbwin: WARNING — BOOTSECT.DAT not generated:");
            eprintln!("    {e:#}");
            eprintln!(
                "usbwin: NTLDR boot.ini menu will render, but selecting \
                 'text mode setup'"
            );
            eprintln!(
                "        will fall through to the default Windows load path \
                 and fail with"
            );
            eprintln!(
                "        '<Windows root>\\\\system32\\\\hal.dll missing'. \
                 Use --boot-record=ms-sys"
            );
            eprintln!("        for a working BOOTSECT.DAT, or wait for bootrec's");
            eprintln!("        single-sector $LDR$ chainloader primitive.");
            eprintln!();
        }
    }

    let _ = diskutil::unmount_disk(&bsd_path);
    diskutil::eject(&bsd_path).context("eject after write")?;

    println!(
        "\nusbwin: {} -> {} (Windows XP mode) OK",
        plan.iso_path.display(),
        info.path
    );
    Ok(())
}

fn apply_txtsetup_mods(usb_mount: &Path, waiters_dir: Option<&Path>) -> Result<()> {
    // FAT32 is case-insensitive but case-preserving. The ISO often has
    // "I386/TXTSETUP.SIF" (uppercase). Try a couple of common spellings.
    let candidates = [
        usb_mount.join("I386").join("TXTSETUP.SIF"),
        usb_mount.join("i386").join("txtsetup.sif"),
        usb_mount.join("I386").join("txtsetup.sif"),
    ];
    let sif_path = candidates
        .iter()
        .find(|p| p.exists())
        .ok_or_else(|| anyhow!("TXTSETUP.SIF not found on USB at any of {candidates:?}; is this really an XP install ISO?"))?;
    let i386_dir = sif_path.parent().unwrap();

    let original = std::fs::read_to_string(sif_path)
        .with_context(|| format!("reading {}", sif_path.display()))?;
    let mut sif = windows_xp_sif::Sif::parse(&original);
    let moved = windows_xp_sif::apply_usb_boot_mods(&mut sif)
        .map_err(|e| anyhow!("apply_usb_boot_mods: {e}"))?;
    if moved == 0 {
        bail!(
            "no USB drivers found in [InputDevicesSupport.Load] of TXTSETUP.SIF; \
             this doesn't look like a standard XP install media. Expected at least \
             one of usbehci/usbohci/usbuhci/usbhub/usbstor."
        );
    }

    // Chunk 6: optionally copy WaitBT.sys / Wait4UFD.sys into I386/ and
    // declare them in the SIF. Use BSD-style copy so timestamps don't matter.
    let mut waiters_installed = 0;
    if let Some(src_dir) = waiters_dir {
        for (key, description) in &[
            ("WaitBT", "USB Boot Wait Driver"),
            ("Wait4UFD", "USB Flash Drive Settle Driver"),
        ] {
            let src = src_dir.join(format!("{key}.sys"));
            let dest = i386_dir.join(format!("{key}.sys"));
            std::fs::copy(&src, &dest)
                .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
            windows_xp_sif::declare_waiter(&mut sif, key, description)
                .map_err(|e| anyhow!("declaring waiter {key} in SIF: {e}"))?;
            waiters_installed += 1;
        }
    }

    let modified = sif.render();
    std::fs::write(sif_path, modified)
        .with_context(|| format!("writing modified {}", sif_path.display()))?;
    println!(
        "usbwin: modified {} (moved {} USB drivers; installed {} waiter drivers)",
        sif_path.display(),
        moved,
        waiters_installed
    );
    Ok(())
}

fn write_unattended(usb_mount: &Path, config: &Config) -> Result<()> {
    let opts = windows_xp_unattended::UnattendedOptions {
        product_key: config.xp_product_key.clone(),
        computer_name: config.xp_computer_name.clone(),
        full_name: config.xp_full_name.clone(),
    };
    let body = windows_xp_unattended::generate(&opts);

    // For USB-bootstrapped setup, winnt.sif lives at the partition root —
    // that's where the WinSetupFromUSB recipe puts it (setupldr.bin looks
    // there first when MsDosInitiated="1"). Also write to I386/ for
    // belt-and-braces compatibility with stock CD-style setup paths.
    let root_dest = usb_mount.join("winnt.sif");
    std::fs::write(&root_dest, &body)
        .with_context(|| format!("writing {}", root_dest.display()))?;
    println!("usbwin: wrote {}", root_dest.display());

    let i386 = xp_staging::find_i386_dir(usb_mount)?;
    let i386_dest = i386.join("winnt.sif");
    std::fs::write(&i386_dest, body)
        .with_context(|| format!("writing {}", i386_dest.display()))?;
    println!("usbwin: wrote {}", i386_dest.display());
    Ok(())
}

/// Read the first sector (512 bytes) of the partition's PBR via raw I/O.
/// Used after a PBR write to derive the BOOTSECT.DAT patched copy.
fn read_pbr_sector0(partition_raw: &str, model: &str) -> Result<Vec<u8>> {
    let mut dev = RawDevice::open(partition_raw, OpenMode::ReadOnly, model)
        .with_context(|| format!("opening {partition_raw} for PBR read-back"))?;
    let mut buf = vec![0u8; 512];
    dev.read_at(0, &mut buf).map_err(anyhow_from_core)?;
    Ok(buf)
}

fn write_mbr_sector(info: &DeviceInfo) -> Result<()> {
    // Use the Win 7 MBR variant even in XP mode. The MBR's job is
    // OS-agnostic (find active partition, chainload its PBR), and
    // MBR_WIN7 is the bytes that boot end-to-end on the Dell E6410
    // reference rig (verified 2026-05-19 for Win 7). MBR_XP's PBR
    // chainload completes on the same hardware too (we saw bootrec's
    // PBR diagnostic print '2'), but if any downstream stage depends
    // on MBR side-effects (segment-register state, DL convention,
    // disk-signature presence), Win7's MBR is the known-quantity.
    let mbr = boot_records::build_mbr_win7(info.size_bytes)?;
    let mut dev = RawDevice::open(&info.path, OpenMode::ReadWrite, &info.model)
        .context("opening whole disk for MBR write")?;
    dev.write_at(0, &mbr).map_err(anyhow_from_core)?;
    dev.sync().map_err(anyhow_from_core)?;
    let mut readback = vec![0u8; 512];
    dev.read_at(0, &mut readback).map_err(anyhow_from_core)?;
    if readback != mbr {
        bail!("MBR write verify mismatch");
    }
    Ok(())
}

/// Splice the multi-sector FAT32 NTLDR PBR over the freshly-formatted
/// partition's reserved area (sectors 0..2). Same shape as the Win 7
/// BOOTMGR splice: preserves BPB at sector 0 + FSInfo at sector 1, lays
/// stage 2 at sector 2.
fn splice_ntldr_pbr(partition_raw: &str, model: &str, verify: bool) -> Result<()> {
    let mut dev = RawDevice::open(partition_raw, OpenMode::ReadWrite, model)
        .with_context(|| format!("opening {partition_raw} for PBR splice"))?;

    let mut existing = vec![0u8; 1024];
    dev.read_at(0, &mut existing).map_err(anyhow_from_core)?;

    let spliced = boot_records::splice_pbr_ntldr(&existing)?;

    dev.write_at(0, &spliced).map_err(anyhow_from_core)?;
    dev.sync().map_err(anyhow_from_core)?;

    if verify {
        let mut readback = vec![0u8; spliced.len()];
        dev.read_at(0, &mut readback).map_err(anyhow_from_core)?;
        if spliced != readback {
            bail!("PBR splice verify mismatch");
        }
    }
    Ok(())
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

// File-copy machinery: same as windows.rs (chunked, MultiProgress with files+bytes).
// Inlined rather than abstracted to keep the v0.3 pipeline standalone; we can
// refactor into a shared module once Linux/UEFI modes also need it.

fn copy_iso_contents(iso_mount: &Path, usb_mount: &Path) -> Result<()> {
    let entries = walk_files(iso_mount)?;
    let total_bytes: u64 = entries.iter().map(|e| e.size).sum();
    let total_files = entries.iter().filter(|e| !e.is_dir).count() as u64;

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
    let _ = std::process::Command::new("sync").status();
    pb_bytes.finish();
    pb_files.finish();
    Ok(())
}

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
        dest_f.write_all(&buf[..n]).context("write to USB")?;
        *bytes_copied += n as u64;
        update_messages(pb_bytes, pb_files, *bytes_copied, total_bytes, files_copied, total_files, start);
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
    let bytes_eta = if bps > 0.0 { (total_bytes - bytes_copied) as f64 / bps } else { 0.0 };
    let files_eta = if fps > 0.0 { (total_files - files_copied) as f64 / fps } else { 0.0 };
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
    if s >= 3600 { format!("{}h{:02}m", s / 3600, (s % 3600) / 60) }
    else if s >= 60 { format!("{}m{:02}s", s / 60, s % 60) }
    else { format!("{}s", s) }
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
            out.push(CopyEntry { rel: rel.clone(), size: 0, is_dir: true });
            walk_recursive(root, &path, out)?;
        } else if metadata.is_file() {
            out.push(CopyEntry { rel, size: metadata.len(), is_dir: false });
        }
    }
    Ok(())
}

fn anyhow_from_core(e: usbwin_core::Error) -> anyhow::Error {
    anyhow!("{e}")
}
