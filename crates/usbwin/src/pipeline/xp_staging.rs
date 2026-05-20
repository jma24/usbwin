//! XP-USB boot-file staging — the WinSetupFromUSB recipe in code.
//!
//! After the ISO has been recursively copied onto the FAT32 partition,
//! XP's boot chain still won't find anything to load because XP install
//! media expects most of its loader chain to live in `\I386\` (a CD-ROM
//! layout assumption). To boot from USB we need extra files at the
//! partition root:
//!
//!   `\NTLDR`              — copy of `\I386\NTLDR`
//!   `\NTDETECT.COM`       — copy of `\I386\NTDETECT.COM`
//!   `\$LDR$`              — copy of `\I386\SETUPLDR.BIN`, renamed
//!   `\boot.ini`           — generated; two entries (text-mode + GUI-mode)
//!   `\$WIN_NT$.~BT\BOOTSECT.DAT`
//!                          — copy of the partition's PBR with the 11-byte
//!                            `NTLDR      ` filename replaced by
//!                            `$LDR$      ` (8.3 padded)
//!
//! NTLDR runs from the PBR, reads `boot.ini`, sees the bootsector entry
//! pointing at `$WIN_NT$.~BT\BOOTSECT.DAT`, loads that file to 0x7C00,
//! and chainloads it. The patched bootsector then loads `$LDR$` (which
//! is `setupldr.bin` under a 5-char-or-less alias), and text-mode setup
//! starts.
//!
//! Canonical recipe in code:
//!   github.com/ruo91/USB_MultiBoot — `USB_MultiBoot_10.cmd` + `makebt/`
//! Authors: jaclaz, wimb, cdob, ilko_t, porear (boot-land / MSFN, 2006-2008).
//! See also docs/V0.3_WINDOWS_XP.md.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use usbwin_core::Device;

use super::fat32;

/// Three-entry boot.ini covering both stages of the XP install plus a
/// destructive-wipe option for prepping dirty target disks:
///
///   - "1st, text mode setup" loads `BOOTSECT.DAT` (raw-LBA $LDR$ loader)
///     via NTLDR's bootsector-entry mechanism → setupldr starts setup.
///   - "2nd, GUI mode setup" boots the installed Windows on the internal
///     disk (rdisk(1)) after text-mode setup has copied files there.
///   - "3rd, wipe internal HDD" loads `WIPE.DAT` (the mkmsbr-supplied
///     wipe_bootsect blob) which prompts for confirmation, then zeros
///     the first 1 MiB of `DL XOR 1` (the non-USB primary disk) to
///     destroy stale MBR / protective MBR / GPT-primary-header bytes
///     so XP's text-mode setup repartitions cleanly. Safe by default:
///     NTLDR's default is still the text-mode entry, the wipe entry
///     requires both selection AND a Y keypress at the wipe prompt.
///
/// **Syntax matters**: bootsector entries use `C:\path` (drive-letter
/// prefix), NOT ARC paths. NTLDR silently rejects malformed bootsector
/// entries and falls through to the next entry — which gave us the
/// classic "<Windows root>\system32\hal.dll missing" symptom from the
/// rdisk(1) fallthrough when we had this wrong. OS entries (the 2nd
/// here) use ARC syntax.
///
/// /SOS /NOGUIBOOT and similar kernel switches do NOT apply to
/// bootsector entries — those flags are kernel-load options. Omitted.
///
/// CRLF line endings; NTLDR is finicky.
///
/// Format per WinSetupFromUSB MakeBS.cmd (jaclaz 2007), confirmed
/// 2026-05-19 by NTLDR successfully chainloading.
pub const BOOT_INI: &str = concat!(
    "[boot loader]\r\n",
    "timeout=10\r\n",
    "default=C:\\$WIN_NT$.~BT\\BOOTSECT.DAT\r\n",
    "\r\n",
    "[operating systems]\r\n",
    "C:\\$WIN_NT$.~BT\\BOOTSECT.DAT=\"1st, text mode setup\"\r\n",
    "multi(0)disk(0)rdisk(1)partition(1)\\WINDOWS=\"2nd, GUI mode setup\"\r\n",
    "C:\\WIPE.DAT=\"3rd, wipe internal HDD (destructive)\"\r\n",
);

/// FAT 8.3-padded filename strings the PBR uses as a literal compare target.
/// Both are 11 bytes: filename + extension padded with spaces.
const NTLDR_PADDED: &[u8; 11] = b"NTLDR      ";
const LDR_PADDED: &[u8; 11] = b"$LDR$      ";

/// Locate the `I386/` directory on a mounted USB. FAT32 stores both an
/// 8.3 and a long-name entry; macOS shows whichever case the formatter
/// saw first. Win XP ISOs are uppercase but be tolerant.
pub fn find_i386_dir(usb_mount: &Path) -> Result<PathBuf> {
    for name in &["I386", "i386"] {
        let p = usb_mount.join(name);
        if p.is_dir() {
            return Ok(p);
        }
    }
    bail!(
        "no I386/ directory at {} — is this really a Windows XP ISO?",
        usb_mount.display()
    )
}

/// Copy `\I386\NTLDR`, `\I386\NTDETECT.COM`, `\I386\SETUPLDR.BIN` (as `$LDR$`),
/// and `\I386\TXTSETUP.SIF` to the partition root, plus write `\boot.ini`.
/// The original I386/ tree stays intact.
///
/// **Why `TXTSETUP.SIF` at root**: most XP-USB-install guides (BartPE,
/// WinSetupFromUSB-clone tutorials, MSFN forum recipes) place this file
/// at the root as a primary lookup location. setupldr's source-discovery
/// logic tries multiple paths and root is one of the early ones — having
/// the file there gives us the shortest path to a successful lookup.
pub fn stage_root_boot_files(usb_mount: &Path, i386: &Path) -> Result<()> {
    // (source name in I386/, destination name at root)
    let copies: &[(&str, &str)] = &[
        ("NTLDR", "NTLDR"),
        ("NTDETECT.COM", "NTDETECT.COM"),
        ("SETUPLDR.BIN", "$LDR$"),
        ("TXTSETUP.SIF", "TXTSETUP.SIF"),
    ];
    for (src_name, dst_name) in copies {
        let src = i386.join(src_name);
        if !src.exists() {
            // Some ISOs (international, slipstreamed) use lowercase.
            let src_lower = i386.join(src_name.to_lowercase());
            if !src_lower.exists() {
                bail!(
                    "expected {} in {} (case-insensitive) but it's missing — \
                     is this a complete XP ISO?",
                    src_name,
                    i386.display()
                );
            }
        }
        let actual_src = if src.exists() {
            src
        } else {
            i386.join(src_name.to_lowercase())
        };
        let dst = usb_mount.join(dst_name);
        std::fs::copy(&actual_src, &dst).with_context(|| {
            format!("copy {} -> {}", actual_src.display(), dst.display())
        })?;
    }

    std::fs::write(usb_mount.join("boot.ini"), BOOT_INI)
        .with_context(|| format!("writing boot.ini to {}", usb_mount.display()))?;

    Ok(())
}

/// Build `BOOTSECT.DAT` bytes by patching a PBR sector: replace the 11-byte
/// `NTLDR      ` filename with `$LDR$      `. The PBR's boot code does a
/// literal byte-compare against this string when scanning the FAT root
/// directory, so changing the bytes changes what file it loads.
///
/// Works against any FAT32 NT5.x PBR variant — bootrec puts the string at
/// offset 0x1AE, ms-sys at offset 368, others elsewhere. We search rather
/// than assume an offset.
pub fn build_bootsect_dat(pbr_sector0: &[u8]) -> Result<Vec<u8>> {
    if pbr_sector0.len() < 512 {
        bail!(
            "PBR sector too short ({} bytes); expected ≥512",
            pbr_sector0.len()
        );
    }
    // BOOTSECT.DAT is a single sector — NTLDR loads it to 0x7C00 and
    // chainloads. So we must find the NTLDR filename in sector 0;
    // matches in later sectors (e.g. bootrec's stage-2 code at sector 2)
    // wouldn't be reached by the loaded bootsector at runtime.
    let sector0 = &pbr_sector0[..512];
    let pos = sector0
        .windows(NTLDR_PADDED.len())
        .position(|w| w == NTLDR_PADDED)
        .ok_or_else(|| {
            anyhow!(
                "NTLDR filename string not found in sector 0 of the PBR. \
                 This PBR variant doesn't embed the filename in sector 0, \
                 so the BOOTSECT.DAT mechanism can't redirect it. \
                 (ms-sys --fat32nt puts it at offset 0x170; bootrec's \
                 NTLDR multi-sector variant puts it at offset 0x5D0 in \
                 stage 2 — which is unreachable from a single-sector \
                 BOOTSECT.DAT load.)"
            )
        })?;
    let mut out = sector0.to_vec();
    out[pos..pos + LDR_PADDED.len()].copy_from_slice(LDR_PADDED);
    Ok(out)
}

/// Produce a single-sector BOOTSECT.DAT by walking FAT to find `$LDR$`,
/// coalescing its LBAs into runs, and asking bootrec for a raw-LBA loader.
///
/// This is the *correct* BOOTSECT.DAT generator (works against any PBR
/// variant since it doesn't rely on the PBR embedding the NTLDR filename
/// in sector 0). The older `build_bootsect_dat(pbr_sector0)` patcher
/// stays as a fallback for the ms-sys PBR path until this becomes
/// universal.
///
/// Currently **blocked on `bootrec::build_xp_setup_chain_bootsect`** —
/// see bootrec/docs/XP_SETUP_CHAIN_BOOTSECT_SPEC.md. Once that ships,
/// uncomment the marked line and delete the `bail!`. Everything else
/// (FAT walk, run coalesce, file extent → runs) is wired up and tested.
pub fn build_chain_bootsect_via_lba(
    partition_device: &mut dyn Device,
) -> Result<Vec<u8>> {
    let mut sector0 = vec![0u8; 512];
    partition_device
        .read_at(0, &mut sector0)
        .map_err(|e| anyhow!("read partition sector 0 for BPB: {e}"))?;

    let bpb = fat32::Bpb::parse(&sector0).context("parsing FAT32 BPB")?;

    let extent = fat32::find_file_extent(partition_device, &bpb, b"$LDR$      ")
        .context("walking FAT for $LDR$")?
        .ok_or_else(|| {
            anyhow!(
                "$LDR$ not found in FAT root after staging — \
                 was stage_root_boot_files run on this partition?"
            )
        })?;

    let runs = fat32::coalesce_lbas_to_runs(&extent.lbas);
    if runs.is_empty() {
        bail!("$LDR$ has no LBAs — empty file?");
    }
    if runs.len() > 8 {
        bail!(
            "$LDR$ is fragmented across {} runs — too many to fit in \
             a 512-byte bootsector. Reformat the partition or stage \
             $LDR$ earlier (it should be one of the first files written).",
            runs.len()
        );
    }

    let sector0_arr: &[u8; 512] = sector0[..512]
        .try_into()
        .expect("sector0 buffer was sized to 512 above");
    let target_segment: u16 = 0x2000; // setupldr.bin's canonical load segment

    let bootrec_runs: Vec<bootrec::LbaRun> = runs
        .iter()
        .map(|r| bootrec::LbaRun {
            start_lba: r.start_lba,
            sector_count: r.sector_count,
        })
        .collect();

    bootrec::build_xp_setup_chain_bootsect(sector0_arr, target_segment, &bootrec_runs)
        .map(|arr| arr.to_vec())
        .map_err(|e| anyhow!("bootrec::build_xp_setup_chain_bootsect: {e}"))
}

/// Move `\I386\` → `\$WIN_NT$.~BT\` via a FAT32 directory-entry rename.
/// No data copy. Setupldr launched via BOOTSECT.DAT chainload reads its
/// source files from `\$WIN_NT$.~BT\` by default, so the I386 tree must
/// live there (txtsetup.sif, biosinfo.inf, kernel, drivers, the
/// `HIVE*.INF` registry seeds — setupdd reads them all from this
/// directory; missing files leave the target SYSTEM hive broken and
/// produce PROCESS1_INITIALIZATION_FAILED 0x6B / 0xC000003A at smss-init).
///
/// Previously we `ditto`'d `\I386\` → `\$WIN_NT$.~BT\` (~580 MB of
/// redundant I/O — the ISO copy already put the tree on the partition
/// at `\I386\`; the FAT directory entry just needs to point at a new
/// name). Rename is instant.
pub fn move_i386_to_bt(usb_mount: &Path) -> Result<()> {
    let i386 = find_i386_dir(usb_mount)?;
    let bt = usb_mount.join("$WIN_NT$.~BT");
    if bt.exists() {
        bail!(
            "{} already exists — pipeline ordering bug? (expected ~BT to \
             be absent before move_i386_to_bt)",
            bt.display()
        );
    }
    std::fs::rename(&i386, &bt)
        .with_context(|| format!("rename {} -> {}", i386.display(), bt.display()))?;
    Ok(())
}

/// Verbatim canonical USB_MultiBoot rename scripts. They live inside
/// `\$WIN_NT$.~LS\I386\` on the USB and are declared in `TXTSETUP.SIF`'s
/// `[SourceDisksFiles]` so text-mode setup knows to copy them; `winnt.sif`
/// then invokes them via `[SetupParams] UserExecute` (at end of text-mode)
/// and `[GuiRunOnce]` (after GUI-mode finishes).
///
/// `ren_fold.cmd` runs *between* text-mode setup and GUI-mode setup. It
/// renames `\$WIN_NT$.~BT` → `WIN_NT.BT` and `\$WIN_NT$.~LS` → `WIN_NT.LS`
/// on the USB so that GUI-mode setup's boot-volume sanity checks don't
/// abort when they see those literal folder names (text-mode setup
/// considers them indicative of a partially-completed install and would
/// otherwise prompt to insert the CD or re-run setup).
///
/// `undoren.cmd` runs via `[GuiRunOnce]` after first login of the freshly-
/// installed Windows: it renames the folders back, leaving the USB stick
/// re-usable for another install.
///
/// Source: github.com/ruo91/USB_MultiBoot, `USB_MultiBoot_10/makebt/`.
/// Authors: ilko_t, wimb, jaclaz, cdob (boot-land / MSFN, 2007-2008).
pub const REN_FOLD_CMD: &[u8] = include_bytes!("xp_assets/ren_fold.cmd");
pub const UNDOREN_CMD: &[u8] = include_bytes!("xp_assets/undoren.cmd");

/// Mirror the contents of `\$WIN_NT$.~BT\` into `\$WIN_NT$.~LS\I386\` on
/// the mounted USB, and place the canonical rename scripts (`ren_fold.cmd`,
/// `undoren.cmd`) at the new directory's root.
///
/// Sources from `~BT` (not `\I386\`) because by this point in the pipeline
/// `\I386\` has been renamed to `~BT` (see `move_i386_to_bt`). The two
/// directories are byte-identical — `~BT` is the I386 tree with a
/// different name. Same content, one fewer redundant copy operation.
///
/// `\$WIN_NT$.~LS\I386\` is what XP GUI-mode setup expects as the install
/// source after text-mode finishes — the name is hard-coded in `setupdd.sys`
/// and is NOT configurable via `SetupSourcePath` when `MsDosInitiated=1`
/// (which our boot chain requires). Without this folder, GUI-mode setup
/// prompts "please insert the Windows XP CD" and the install stalls.
///
/// During text-mode setup, XP copies the contents of `\$WIN_NT$.~LS\I386\`
/// from the install media to `C:\$WIN_NT$.~LS\I386\` on the target HDD,
/// so the post-reboot GUI-mode setup reads from local disk regardless of
/// whether the USB stick is still attached or has shifted drive letters.
pub fn stage_ls_from_bt(usb_mount: &Path) -> Result<()> {
    let bt = usb_mount.join("$WIN_NT$.~BT");
    if !bt.is_dir() {
        bail!(
            "expected {} to exist (run move_i386_to_bt first)",
            bt.display()
        );
    }
    let ls = usb_mount.join("$WIN_NT$.~LS");
    let ls_i386 = ls.join("I386");
    std::fs::create_dir_all(&ls_i386)
        .with_context(|| format!("creating {}", ls_i386.display()))?;

    let status = std::process::Command::new("ditto")
        .arg(&bt)
        .arg(&ls_i386)
        .status()
        .with_context(|| {
            format!("invoking ditto {} {}", bt.display(), ls_i386.display())
        })?;
    if !status.success() {
        bail!(
            "ditto {} {} failed with {status}",
            bt.display(),
            ls_i386.display()
        );
    }

    // Place the rename scripts inside the replicated I386 folder. XP's
    // `[SourceDisksFiles]` directive `100,,,,,,_x,2,0,0` looks for them at
    // exactly this path on the install media.
    let ren_fold = ls_i386.join("ren_fold.cmd");
    std::fs::write(&ren_fold, REN_FOLD_CMD)
        .with_context(|| format!("writing {}", ren_fold.display()))?;
    let undoren = ls_i386.join("undoren.cmd");
    std::fs::write(&undoren, UNDOREN_CMD)
        .with_context(|| format!("writing {}", undoren.display()))?;

    Ok(())
}

/// Write `\$WIN_NT$.~BT\BOOTSECT.DAT` to the mounted USB. Creates the
/// directory if missing.
pub fn write_bootsect_dat(usb_mount: &Path, bytes: &[u8]) -> Result<()> {
    let dir = usb_mount.join("$WIN_NT$.~BT");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let dest = dir.join("BOOTSECT.DAT");
    std::fs::write(&dest, bytes).with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

/// Write the mkmsbr-supplied wipe-bootsector blob to `\WIPE.DAT` at the
/// partition root. NTLDR's boot.ini third entry chainloads this file
/// (see [`BOOT_INI`]). The blob is a single 512-byte real-mode bootsector
/// that:
///
///   1. Captures BIOS-supplied DL (= USB drive — never touched).
///   2. Probes target = `DL XOR 1` with INT 13h fn 0x41; aborts if absent.
///   3. Reads target's size via INT 13h fn 0x48 and prints
///      `target=0xNN size=NNNN MiB / USB=0xNN safe`.
///   4. Requires `Y`/`y` to confirm; anything else cancels.
///   5. On confirm, zeros LBA 0..2047 (1 MiB) of the target via INT 13h
///      fn 0x43 then reboots via INT 19h.
///
/// Source: `mkmsbr/boot-asm/wipe_bootsect.asm`, exported as
/// `bootrec::WIPE_BOOTSECT_BOOT`.
pub fn stage_wipe_bootsect(usb_mount: &Path) -> Result<()> {
    let bytes = bootrec::WIPE_BOOTSECT_BOOT;
    if bytes.len() != 512 {
        bail!(
            "WIPE_BOOTSECT_BOOT is {} bytes; expected exactly 512 \
             (rebuild mkmsbr with --features embed-boot-asm)",
            bytes.len()
        );
    }
    if bytes[510] != 0x55 || bytes[511] != 0xAA {
        bail!("WIPE_BOOTSECT_BOOT missing 0x55 0xAA boot signature");
    }
    let dest = usb_mount.join("WIPE.DAT");
    std::fs::write(&dest, bytes)
        .with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_ini_has_all_three_entries() {
        assert!(BOOT_INI.contains("1st, text mode setup"));
        assert!(BOOT_INI.contains("2nd, GUI mode setup"));
        assert!(BOOT_INI.contains("3rd, wipe internal HDD"));
        // Bootsector entry uses drive-letter syntax — NOT ARC path.
        // NTLDR rejects ARC-pathed bootsector entries silently.
        assert!(
            BOOT_INI.contains("C:\\$WIN_NT$.~BT\\BOOTSECT.DAT="),
            "text-mode bootsector entry must use C:\\path syntax"
        );
        assert!(
            BOOT_INI.contains("C:\\WIPE.DAT="),
            "wipe bootsector entry must use C:\\path syntax"
        );
        assert!(
            !BOOT_INI.contains("multi(0)disk(0)rdisk(0)partition(1)\\$WIN_NT$"),
            "ARC path for bootsector entry would be rejected by NTLDR"
        );
        // Default must be the text-mode entry, NOT the destructive wipe
        // entry — accidental Enter at the menu must not nuke a disk.
        assert!(BOOT_INI.contains("default=C:\\$WIN_NT$.~BT\\BOOTSECT.DAT"));
        assert!(
            !BOOT_INI.contains("default=C:\\WIPE.DAT"),
            "wipe entry MUST NOT be the boot.ini default"
        );
        // Kernel switches don't apply to bootsector entries; should be absent.
        assert!(!BOOT_INI.contains("/SOS"));
        // CRLF line endings (NTLDR is picky).
        assert!(BOOT_INI.contains("\r\n"));
        assert_eq!(
            BOOT_INI.matches('\n').count(),
            BOOT_INI.matches("\r\n").count(),
            "every LF must be preceded by CR"
        );
    }

    #[test]
    fn wipe_bootsect_blob_is_valid() {
        let bytes = bootrec::WIPE_BOOTSECT_BOOT;
        assert_eq!(
            bytes.len(),
            512,
            "wipe bootsector must be exactly 512 bytes"
        );
        assert_eq!(bytes[510], 0x55, "boot signature byte 0 must be 0x55");
        assert_eq!(bytes[511], 0xAA, "boot signature byte 1 must be 0xAA");
        // First instruction should be CLI (0xFA) — the bootsector's first
        // act is to disable interrupts before setting up segment regs.
        assert_eq!(bytes[0], 0xFA, "first byte should be CLI (0xFA)");
        // Source string sanity — the user-visible label confirming
        // the right blob ended up here.
        let body = &bytes[..510];
        assert!(
            body.windows(11).any(|w| w == b"USBWIN WIPE"),
            "expected 'USBWIN WIPE' marker string in wipe_bootsect"
        );
    }

    #[test]
    fn stage_wipe_bootsect_writes_512_bytes_at_root() {
        let tmp = std::env::temp_dir().join("usbwin_stage_wipe_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        stage_wipe_bootsect(&tmp).unwrap();
        let dest = tmp.join("WIPE.DAT");
        let written = std::fs::read(&dest).unwrap();
        assert_eq!(written.len(), 512);
        assert_eq!(written[510], 0x55);
        assert_eq!(written[511], 0xAA);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn build_bootsect_dat_patches_at_planted_offset() {
        let mut pbr = vec![0u8; 512];
        pbr[100..111].copy_from_slice(NTLDR_PADDED);
        let patched = build_bootsect_dat(&pbr).unwrap();
        assert_eq!(&patched[100..111], LDR_PADDED);
        // Nothing else changed.
        assert_eq!(&patched[..100], &pbr[..100]);
        assert_eq!(&patched[111..], &pbr[111..]);
    }

    #[test]
    fn build_bootsect_dat_errors_if_no_ntldr() {
        let pbr = vec![0u8; 512];
        let err = build_bootsect_dat(&pbr).unwrap_err();
        assert!(
            err.to_string().contains("not found in sector 0"),
            "got: {err}"
        );
    }

    #[test]
    fn build_bootsect_dat_errors_on_short_input() {
        let pbr = vec![0u8; 100];
        let err = build_bootsect_dat(&pbr).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn build_bootsect_dat_rejects_bootrec_ntldr_pbr() {
        // bootrec's NTLDR multi-sector PBR puts the literal "NTLDR" string
        // in stage 2 (sector 2, offset 0x5D0), not sector 0. The BOOTSECT.DAT
        // mechanism only loads sector 0, so we can't redirect to $LDR$ via
        // a sector-0 byte patch. Until bootrec ships an NTLDR PBR variant
        // with the filename in sector 0, this combination is unsupported.
        let golden = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/golden/pbr_ntldr_multi.bin"
        ))
        .expect(
            "pbr_ntldr_multi.bin golden missing — \
             run UPDATE_GOLDENS=1 cargo test -p usbwin --bin usbwin boot_records",
        );
        let err = build_bootsect_dat(&golden).unwrap_err();
        assert!(
            err.to_string().contains("not found in sector 0"),
            "expected sector-0-not-found error, got: {err}"
        );
    }

    #[test]
    fn build_bootsect_dat_works_on_mssys_style_pbr() {
        // ms-sys --fat32nt embeds the NTLDR filename at offset 0x170
        // (368 decimal) in sector 0 — confirmed empirically from the
        // 2026-05-19 byte dump. Synthesize a PBR with that layout.
        let mut pbr = vec![0u8; 1024];
        pbr[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        pbr[368..379].copy_from_slice(NTLDR_PADDED);
        pbr[510..512].copy_from_slice(&[0x55, 0xAA]);

        let patched = build_bootsect_dat(&pbr).unwrap();
        assert_eq!(patched.len(), 512);
        assert_eq!(&patched[368..379], LDR_PADDED);
        assert_eq!(&patched[510..512], &[0x55, 0xAA]);
    }

    #[test]
    fn find_i386_dir_works_on_real_fixture() {
        // Use the xp_sp3 fixture directory's parent as a synthetic mount.
        let tmp = std::env::temp_dir().join("usbwin_xp_staging_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("I386")).unwrap();
        assert_eq!(find_i386_dir(&tmp).unwrap(), tmp.join("I386"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
