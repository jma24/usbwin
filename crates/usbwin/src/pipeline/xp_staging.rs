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

/// Two-entry boot.ini covering both stages of the XP install:
///
///   - "1st, text mode setup" loads `BOOTSECT.DAT` (raw-LBA $LDR$ loader)
///     via NTLDR's bootsector-entry mechanism → setupldr starts setup.
///   - "2nd, GUI mode setup" boots the installed Windows on the internal
///     disk (rdisk(1)) after text-mode setup has copied files there.
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

/// Byte-patch `$LDR$` so setupldr looks for its source files in `\I386\`
/// instead of `\$WIN_NT$.~BT\`.
///
/// **Currently unused** — superseded by `replicate_i386_to_bt` which
/// puts files where setupldr already looks (no setupldr modification).
/// Kept (with tests) because (a) it's a documented approach that may be
/// useful for other consumers, and (b) it's the lighter-weight option if
/// we ever solve the FAT-walker-vs-padded-path interaction that made it
/// not work in our pipeline.
#[allow(dead_code)]
///
/// When setupldr.bin is loaded via the BOOTSECT.DAT chainload path (as
/// opposed to a CD-style direct boot), its source-detection logic picks
/// `\$WIN_NT$.~BT\` as the install-files directory. But our USB has the
/// I386 tree at `\I386\` (the natural XP-ISO layout) and only stages
/// `BOOTSECT.DAT` in `\$WIN_NT$.~BT\` — so setupldr fails with the
/// classic "INF file txtsetup.sif is corrupt or missing, status 18".
///
/// Patch: replace every literal `$WIN_NT$.~BT` byte sequence (12 bytes)
/// with `I386` + 8 trailing spaces (4 bytes name + 8 bytes 0x20). Same
/// length, no offset shifts.
///
/// Why spaces and not nulls: empirically, NULL padding produces paths
/// like `\I386\0\0...\0\0\txtsetup.sif` when setupldr uses fixed-size
/// memcpy to build the full path. Spaces are tolerated by FAT short-
/// name matching (trailing spaces in 8.3 names are trimmed for compare)
/// and by setupldr's path-construction code. This matches gsar's
/// default replace behavior, which is what the canonical WinSetupFromUSB
/// patch (jaclaz/wimb, boot-land 2007-2008) actually emits.
pub fn patch_setupldr_for_i386_lookup(ldr_path: &Path) -> Result<usize> {
    const NEEDLE: &[u8; 12] = b"$WIN_NT$.~BT";
    const REPLACEMENT: &[u8; 12] = b"I386        ";

    let mut bytes =
        std::fs::read(ldr_path).with_context(|| format!("reading {}", ldr_path.display()))?;

    let mut patches = 0usize;
    let mut pos = 0;
    while pos + NEEDLE.len() <= bytes.len() {
        if let Some(rel) = bytes[pos..]
            .windows(NEEDLE.len())
            .position(|w| w == &NEEDLE[..])
        {
            let abs = pos + rel;
            bytes[abs..abs + REPLACEMENT.len()].copy_from_slice(REPLACEMENT);
            patches += 1;
            pos = abs + REPLACEMENT.len();
        } else {
            break;
        }
    }

    if patches == 0 {
        bail!(
            "no occurrences of $WIN_NT$.~BT found in {} — wrong file, or \
             setupldr from an XP variant that uses a different path \
             literal? Patch is required for setupldr to locate \\I386\\ \
             when launched via BOOTSECT.DAT.",
            ldr_path.display()
        );
    }

    std::fs::write(ldr_path, &bytes)
        .with_context(|| format!("writing patched {}", ldr_path.display()))?;
    Ok(patches)
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

/// Mirror the contents of `\I386\` into `\$WIN_NT$.~LS\I386\` on the mounted
/// USB, and place the canonical rename scripts (`ren_fold.cmd`,
/// `undoren.cmd`) at the new directory's root.
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
///
/// Cost: another ~580 MB on the stick on top of the `~BT` replica. Fine on
/// any modern flash drive; not worth optimising until the byte-patch route
/// is solved.
pub fn replicate_i386_to_ls(usb_mount: &Path) -> Result<()> {
    let i386 = find_i386_dir(usb_mount)?;
    let ls = usb_mount.join("$WIN_NT$.~LS");
    let ls_i386 = ls.join("I386");
    std::fs::create_dir_all(&ls_i386)
        .with_context(|| format!("creating {}", ls_i386.display()))?;

    let status = std::process::Command::new("ditto")
        .arg(&i386)
        .arg(&ls_i386)
        .status()
        .with_context(|| {
            format!("invoking ditto {} {}", i386.display(), ls_i386.display())
        })?;
    if !status.success() {
        bail!(
            "ditto {} {} failed with {status}",
            i386.display(),
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

/// Mirror the contents of `\I386\` into `\$WIN_NT$.~BT\` on the mounted
/// USB. Setupldr launched via BOOTSECT.DAT chain looks for its source
/// files under `\$WIN_NT$.~BT\` by default; without this, every lookup
/// (`txtsetup.sif`, `biosinfo.inf`, the kernel, every driver) misses
/// and setupldr halts with "txtsetup.sif corrupt or missing, status 18".
///
/// We attempted to byte-patch `$LDR$`'s `$WIN_NT$.~BT` literal to `I386`
/// (the WinSetupFromUSB recipe via gsar.exe) but FAT short-name lookup
/// against the resulting 12-char-with-spaces path component fails on
/// our partition. Replicating the directory is simpler and works
/// regardless of padding strategy / walker quirks. Cost: ~580 MB extra
/// on the stick (XP install ISO doubles its disk footprint). On a 64 GB
/// stick this is fine.
///
/// Implementation: shell out to `ditto` (macOS native recursive copy
/// with copy_file_range under the hood — much faster than std::fs).
pub fn replicate_i386_to_bt(usb_mount: &Path) -> Result<()> {
    let i386 = find_i386_dir(usb_mount)?;
    let bt = usb_mount.join("$WIN_NT$.~BT");
    std::fs::create_dir_all(&bt)
        .with_context(|| format!("creating {}", bt.display()))?;

    let status = std::process::Command::new("ditto")
        .arg(&i386)
        .arg(&bt)
        .status()
        .with_context(|| format!("invoking ditto {} {}", i386.display(), bt.display()))?;
    if !status.success() {
        bail!(
            "ditto {} {} failed with {status}",
            i386.display(),
            bt.display()
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_ini_has_both_entries() {
        assert!(BOOT_INI.contains("1st, text mode setup"));
        assert!(BOOT_INI.contains("2nd, GUI mode setup"));
        // Bootsector entry uses drive-letter syntax — NOT ARC path.
        // NTLDR rejects ARC-pathed bootsector entries silently.
        assert!(
            BOOT_INI.contains("C:\\$WIN_NT$.~BT\\BOOTSECT.DAT="),
            "bootsector entry must use C:\\path syntax"
        );
        assert!(
            !BOOT_INI.contains("multi(0)disk(0)rdisk(0)partition(1)\\$WIN_NT$"),
            "ARC path for bootsector entry would be rejected by NTLDR"
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
    fn patch_setupldr_replaces_all_occurrences() {
        let tmp = std::env::temp_dir().join("usbwin_xp_staging_patch_test.bin");
        let _ = std::fs::remove_file(&tmp);

        // Synthesize a fake $LDR$ with the string at two known offsets,
        // plus surrounding junk that must not change.
        let mut blob = vec![0xCCu8; 200];
        blob[10..22].copy_from_slice(b"$WIN_NT$.~BT");
        blob[100..112].copy_from_slice(b"$WIN_NT$.~BT");
        std::fs::write(&tmp, &blob).unwrap();

        let n = patch_setupldr_for_i386_lookup(&tmp).unwrap();
        assert_eq!(n, 2, "should patch both occurrences");

        let patched = std::fs::read(&tmp).unwrap();
        // First 4 bytes of each patched region are "I386", next 8 are spaces.
        assert_eq!(&patched[10..14], b"I386");
        assert_eq!(&patched[14..22], b"        ");
        assert_eq!(&patched[100..104], b"I386");
        assert_eq!(&patched[104..112], b"        ");
        // Surrounding bytes unchanged.
        assert_eq!(&patched[0..10], &[0xCC; 10]);
        assert_eq!(&patched[22..100], &[0xCC; 78]);
        assert_eq!(&patched[112..200], &[0xCC; 88]);

        std::fs::remove_file(&tmp).unwrap();
    }

    #[test]
    fn patch_setupldr_errors_if_not_found() {
        let tmp = std::env::temp_dir().join("usbwin_xp_staging_patch_test2.bin");
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(&tmp, vec![0xAA; 100]).unwrap();
        let err = patch_setupldr_for_i386_lookup(&tmp).unwrap_err();
        assert!(err.to_string().contains("no occurrences"));
        std::fs::remove_file(&tmp).unwrap();
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
