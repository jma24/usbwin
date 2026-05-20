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

    // Trim the ISO-root extras the XP install path never reads (DOCS,
    // DOTNETFX, VALUEADD, SUPPORT, SETUP.EXE, *.HTM — ~500 MB). The
    // WIN51* tag files and AUTORUN.INF stay (tiny; some BIOSes use the
    // tag files to recognise install media). Done early so subsequent
    // stages aren't slowed by the extra files sitting on the partition.
    // \I386\ stays for now — apply_txtsetup_mods + stage_root_boot_files
    // read from it, and it gets renamed to \$WIN_NT$.~BT\ below.
    let trim_extras: &[&str] = &[
        "DOCS",
        "DOTNETFX",
        "VALUEADD",
        "SUPPORT",
        "SETUP.EXE",
        "README.HTM",
        "SETUPXP.HTM",
    ];
    for name in trim_extras {
        let path = usb_mount.join(name);
        if path.is_dir() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("removing {}", path.display()))?;
        } else if path.is_file() {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing {}", path.display()))?;
        }
    }
    println!("usbwin: trimmed ISO-root extras (DOCS/DOTNETFX/VALUEADD/SUPPORT/etc, ~500 MB)");

    // Apply the WinSetupFromUSB txtsetup.sif modification on the copied file.
    let sif_moved = apply_txtsetup_mods(&usb_mount, config.xp_waiters_dir.as_deref())
        .context("modifying TXTSETUP.SIF on USB")?;

    // Always write winnt.sif — the GUI-mode-CDROM-prompt fix relies on
    // [SetupParams] UserExecute + [GuiRunOnce] hooks declared there, which
    // need to be present regardless of whether the user asked for a full
    // unattended install.
    write_unattended(&usb_mount, config).context("writing winnt.sif")?;

    // Stage \NTLDR, \NTDETECT.COM, \$LDR$, \boot.ini at the partition root.
    // Without these, the PBR loads \NTLDR (which isn't there yet — XP ISOs
    // keep it in \I386\) and boots fail before NTLDR even runs. WinSetupFromUSB
    // recipe; see pipeline/xp_staging.rs.
    let i386 = xp_staging::find_i386_dir(&usb_mount)?;
    xp_staging::stage_root_boot_files(&usb_mount, &i386)
        .context("staging XP boot files at root")?;
    println!("usbwin: staged NTLDR, NTDETECT.COM, $LDR$, boot.ini at USB root");

    // Move \I386\ → \$WIN_NT$.~BT\ via FAT32 directory-entry rename
    // (instant, no I/O). Setupldr launched via BOOTSECT.DAT chainload
    // reads from ~BT; renaming is cheaper than the ~580 MB ditto we
    // used to do. setupdd reads HIVE*.INF and other non-FloppyFiles
    // entries from this directory too, so we mirror the full I386 set
    // here (a slim DOSNET-based subset was tried 2026-05-20 and produced
    // PROCESS1_INITIALIZATION_FAILED 0x6B / 0xC000003A at smss-init).
    xp_staging::move_i386_to_bt(&usb_mount)
        .context("renaming I386 → $WIN_NT$.~BT")?;
    println!("usbwin: renamed \\I386\\ → \\$WIN_NT$.~BT\\ (FAT32 rename, no I/O)");

    // Stage `\$WIN_NT$.~LS\I386\` (the GUI-mode setup source) and place
    // the canonical ren_fold.cmd / undoren.cmd rename scripts inside it.
    // Text-mode setup copies the contents of this folder to the target HDD
    // as `C:\$WIN_NT$.~LS\I386\`, which is where GUI-mode setup reads from
    // — sidestepping the "please insert the Windows XP CD" prompt that
    // appears when GUI-mode can't find its source on a removable drive.
    // Sources from ~BT (= the renamed \I386\) since they're byte-identical.
    println!("usbwin: replicating $WIN_NT$.~BT → $WIN_NT$.~LS\\I386 (~580 MB, fast via ditto)…");
    xp_staging::stage_ls_from_bt(&usb_mount)
        .context("staging $WIN_NT$.~LS\\I386 from ~BT")?;
    println!("usbwin: staged $WIN_NT$.~LS\\I386 (with ren_fold.cmd, undoren.cmd)");

    // Belt-and-suspenders: re-verify the SIF mod persisted in all three
    // on-disk copies after the rename + ditto. The immediate post-write
    // check in apply_txtsetup_mods proves the bytes were correct on disk
    // before the rename; this proves the rename and ditto preserved them.
    verify_all_sif_copies(&usb_mount, sif_moved).context("verifying SIF copies after staging")?;

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

    // Generate \$WIN_NT$.~BT\BOOTSECT.DAT. Two strategies, tried in order:
    //
    //  (1) Raw-LBA loader via bootrec primitive (preferred). usbwin walks
    //      FAT to find $LDR$'s LBAs, asks bootrec to emit a 512-byte loader
    //      that CHS-reads them and jumps. Works against any PBR variant.
    //      Blocked on bootrec::build_xp_setup_chain_bootsect — currently
    //      returns Err so we fall through.
    //
    //  (2) Patch a copy of the on-disk PBR: replace the 11-byte NTLDR
    //      filename with $LDR$. Works for ms-sys's --fat32nt PBR (NTLDR
    //      string in sector 0). Fails for bootrec's NTLDR multi-sector PBR
    //      (string in stage 2 / sector 2, unreachable).
    //
    // If both fail: log a loud warning and skip the file. NTLDR's boot.ini
    // menu still renders; selecting the text-mode entry falls through to
    // the default Windows-load path and shows '<Windows root>\\system32
    // \\hal.dll missing'. Useful intermediate state for debugging.
    let (bootsect_dat, bootsect_source) = {
        let mut dev = RawDevice::open(&partition_raw, OpenMode::ReadOnly, &info.model)
            .with_context(|| format!("opening {partition_raw} for BOOTSECT.DAT generation"))?;
        let lba_attempt = xp_staging::build_chain_bootsect_via_lba(&mut dev);
        drop(dev);
        match lba_attempt {
            Ok(bytes) => (Ok(bytes), "raw-LBA $LDR$ loader (bootrec primitive)"),
            Err(lba_err) => {
                let pbr_bytes = read_pbr_sector0(&partition_raw, &info.model)
                    .context("reading PBR back for BOOTSECT.DAT fallback")?;
                let attempt = xp_staging::build_bootsect_dat(&pbr_bytes).map_err(|patch_err| {
                    anyhow!(
                        "both BOOTSECT.DAT strategies failed:\n  \
                         raw-LBA path: {lba_err:#}\n  \
                         PBR-patch path: {patch_err:#}"
                    )
                });
                (attempt, "PBR-patch fallback (NTLDR→$LDR$ replace)")
            }
        }
    };

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
                "usbwin: wrote {}/$WIN_NT$.~BT/BOOTSECT.DAT ({} bytes, {})",
                usb_mount2.display(),
                bytes.len(),
                bootsect_source,
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

/// After all the file staging (root copy + I386→~BT rename + ditto to ~LS),
/// re-read each TXTSETUP.SIF copy on disk and re-run the persistence check.
/// `expected_moved` is the count the original apply_usb_boot_mods returned,
/// so a copy that's lost or gained a driver line gets caught.
fn verify_all_sif_copies(usb_mount: &Path, expected_moved: usize) -> Result<()> {
    let copies: &[&str] = &[
        "TXTSETUP.SIF",
        "$WIN_NT$.~BT/TXTSETUP.SIF",
        "$WIN_NT$.~LS/I386/TXTSETUP.SIF",
    ];
    for rel in copies {
        let path = usb_mount.join(rel);
        let bytes = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {} for verification", path.display()))?;
        let sif = windows_xp_sif::Sif::parse(&bytes);
        windows_xp_sif::verify_usb_boot_mods_persisted(&sif, expected_moved).map_err(|e| {
            anyhow!(
                "SIF copy at {} failed post-staging verification: {e}",
                path.display()
            )
        })?;
    }
    println!(
        "usbwin: verified TXTSETUP.SIF mods present in all {} on-disk copies",
        copies.len()
    );
    Ok(())
}

fn apply_txtsetup_mods(usb_mount: &Path, waiters_dir: Option<&Path>) -> Result<usize> {
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

    // Declare the rename scripts in [SourceDisksFiles] so text-mode setup
    // recognises them as install-media files. Required for the
    // `UserExecute` / `GuiRunOnce` hooks in winnt.sif to actually find
    // their target binaries.
    windows_xp_sif::declare_ren_scripts(&mut sif)
        .map_err(|e| anyhow!("declare_ren_scripts: {e}"))?;

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

    // Re-read the file we just wrote and assert the move actually persisted.
    // Belt-and-suspenders against silent FAT32 / mount-point write failures —
    // the in-memory tests prove the transform is correct but can't prove the
    // bytes reached the platter under macOS's USB FAT32 stack. If this ever
    // fires, the 2026-05-19 "moved 5 but grep finds nothing" report from
    // docs/TECH_DEBT.md was a real bug and we now have hard evidence.
    let on_disk = std::fs::read_to_string(sif_path)
        .with_context(|| format!("re-reading {} for persistence check", sif_path.display()))?;
    let on_disk_sif = windows_xp_sif::Sif::parse(&on_disk);
    windows_xp_sif::verify_usb_boot_mods_persisted(&on_disk_sif, moved).map_err(|e| {
        anyhow!(
            "post-write verification of {} failed: {e}",
            sif_path.display()
        )
    })?;

    println!(
        "usbwin: modified {} (moved {} USB drivers; installed {} waiter drivers; verified on disk)",
        sif_path.display(),
        moved,
        waiters_installed
    );
    Ok(moved)
}

fn write_unattended(usb_mount: &Path, config: &Config) -> Result<()> {
    let body = if config.xp_unattended {
        let opts = windows_xp_unattended::UnattendedOptions {
            product_key: config.xp_product_key.clone(),
            computer_name: config.xp_computer_name.clone(),
            full_name: config.xp_full_name.clone(),
        };
        windows_xp_unattended::generate(&opts)
    } else {
        // Minimum winnt.sif: just the rename-script hooks + MsDosInitiated.
        // GUI-mode prompts still appear (EULA, product key, naming) but the
        // "please insert the Windows XP CD" prompt does NOT, which is the
        // whole point.
        windows_xp_unattended::generate_minimal()
    };

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
