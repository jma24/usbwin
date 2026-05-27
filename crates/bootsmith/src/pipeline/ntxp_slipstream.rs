//! Slipstream a third-party storage driver into `I386\TXTSETUP.SIF`.
//!
//! XP text-mode setup auto-binds drivers listed in TXTSETUP.SIF's
//! `[HardwareIdsDatabase]` to matching PCI devices during enumeration --
//! treating them as if they were inbox drivers. By adding our user-
//! supplied F6 driver to four specific sections of the install source's
//! TXTSETUP.SIF, we make iaStor (or any other AHCI/RAID driver) load
//! without the user pressing F6 + S.
//!
//! Sections patched (each gets new lines inserted at the end of its
//! existing body, before any trailing blank lines):
//!
//! - `[SCSI.Load]` -- `<service> = <driver>.sys,4`
//! - `[SCSI]` -- `<service> = "<human-readable name>"`
//! - `[HardwareIdsDatabase]` -- `PCI\VEN_xxxx&DEV_yyyy... = "<service>"`
//!   (one line per HwID; from the user's TXTSETUP.OEM)
//! - `[SourceDisksFiles]` -- one line per driver file we slipstream,
//!   formatted per file extension; values follow the canonical recipe
//!   from Tim's F6 driver guide / nLite output.
//!
//! Everything else is preserved byte-for-byte (CRLF, comments, blank
//! lines, every existing entry).

use anyhow::{anyhow, bail, Result};

/// Output of section-patching: the rewritten TXTSETUP.SIF text plus a
/// summary of what was added (for the burn-time log line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patched {
    pub text: String,
    pub additions: PatchedSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchedSummary {
    pub service: String,
    pub display_name: String,
    pub files: Vec<String>,
    pub hardware_id_count: usize,
}

/// Patch a TXTSETUP.SIF text by inserting new lines into the four
/// driver-registration sections.
///
/// - `service` is the NT service identifier (e.g. `"iaStor"`). It must
///   match the third field of every `[Files.scsi.X] driver=` line in
///   the user's TXTSETUP.OEM and is what `[HardwareIdsDatabase]` maps
///   PCI ids to.
/// - `display_name` is a free-form string shown in any UI that lists
///   the loaded driver. Conventionally the chipset family.
/// - `files` is every file we're slipstreaming alongside this driver
///   (the .sys plus every referenced .inf and .cat). Each file's
///   `[SourceDisksFiles]` line is formatted based on extension.
/// - `hardware_ids` is `(hwid, service)` pairs lifted from the user's
///   TXTSETUP.OEM `[HardwareIds.scsi.*]` blocks. Pseudo ids that don't
///   look like PCI (e.g. FiraDisk's `detected\firadisk`) are dropped --
///   only PCI bus ids belong in the inbox HwID database.
pub fn patch_txtsetup_sif(
    sif: &str,
    service: &str,
    display_name: &str,
    files: &[String],
    hardware_ids: &[(String, String)],
) -> Result<Patched> {
    if service.is_empty() {
        bail!("slipstream service name is empty");
    }
    if display_name.is_empty() {
        bail!("slipstream display name is empty");
    }
    let driver_sys = files
        .iter()
        .find(|f| f.to_ascii_lowercase().ends_with(".sys"))
        .ok_or_else(|| anyhow!("slipstream file list has no .sys driver"))?
        .clone();

    let hardware_ids_pci: Vec<&(String, String)> = hardware_ids
        .iter()
        .filter(|(h, _)| h.to_ascii_uppercase().starts_with("PCI\\"))
        .collect();
    if hardware_ids_pci.is_empty() {
        bail!("slipstream HwID list has no PCI entries; nothing for PnP to match against");
    }

    let mut additions = SectionAdditions::default();
    additions
        .scsi_load
        .push(format!("{service} = {driver_sys},4"));
    additions
        .scsi
        .push(format!("{service} = \"{display_name}\""));
    for (hwid, svc) in &hardware_ids_pci {
        // Existing XP entries in [HardwareIdsDatabase] use UNQUOTED LHS
        // (the HwID) and quoted RHS (the service name). Wrapping the
        // LHS in literal `"..."` would make XP's parser compare against
        // a HwID string that includes the quote chars, and the PnP
        // match silently fails. The downstream "iaStor.sys could not
        // be found" error is the misleading XP wording for "no HwID
        // matched any registered driver" -- it's not actually about
        // file presence.
        additions
            .hardware_ids_database
            .push(format!("{hwid} = \"{svc}\""));
    }
    for file in files {
        additions
            .source_disks_files
            .push(format_source_disks_files_line(file)?);
    }

    let patched = apply_additions(sif, &additions)?;
    Ok(Patched {
        text: patched,
        additions: PatchedSummary {
            service: service.to_string(),
            display_name: display_name.to_string(),
            files: files.to_vec(),
            hardware_id_count: hardware_ids_pci.len(),
        },
    })
}

#[derive(Default)]
struct SectionAdditions {
    scsi_load: Vec<String>,
    scsi: Vec<String>,
    hardware_ids_database: Vec<String>,
    source_disks_files: Vec<String>,
}

impl SectionAdditions {
    fn for_section(&self, name_lower: &str) -> Option<&[String]> {
        match name_lower {
            "scsi.load" => Some(&self.scsi_load),
            "scsi" => Some(&self.scsi),
            "hardwareidsdatabase" => Some(&self.hardware_ids_database),
            "sourcedisksfiles" => Some(&self.source_disks_files),
            _ => None,
        }
    }
}

/// `[SourceDisksFiles]` line format depends on file extension.
///
/// Field 1 (source disk ID) is `1` -- the base CD's `\I386` directory.
/// That is where `append_file_to_i386` physically writes the unpacked
/// driver files. XP SP3 also has a `100` source disk for service-pack
/// overlay files, but newly slipstreamed files do not live in that
/// overlay set. Registering appended files as disk `100` makes setup
/// ask for `iaStor.sys` even though the file is present in `I386`.
///
/// Field 7 is the on-source filename. XP CD ships drivers
/// makecab-compressed (e.g. `ATAPI.SY_`) and uses a value like `4_` --
/// the trailing `_` triggers the standard compression transform. We
/// slipstream uncompressed binaries, so we use the `_x` token: "use
/// the dictionary key verbatim, file is uncompressed."
///
/// - `.sys` -> `1,,,,,,_x,4,1,,,1,4`
///   dest dir 4 (`system32\drivers`), mandatory upgrade, trailing
///   boot-critical flags so text-mode setup pre-copies the driver into
///   the ramdrive before file-copy phase.
/// - `.inf` / `.cat` -> `1,,,,,,_x,20,0,0`
///   dest dir 20 (`inf`), passive upgrade, no boot-criticality.
fn format_source_disks_files_line(file: &str) -> Result<String> {
    let lower = file.to_ascii_lowercase();
    let format = if lower.ends_with(".sys") {
        "1,,,,,,_x,4,1,,,1,4"
    } else if lower.ends_with(".inf") || lower.ends_with(".cat") {
        "1,,,,,,_x,20,0,0"
    } else {
        bail!("slipstream file {file} has an unsupported extension (expected .sys / .inf / .cat)");
    };
    Ok(format!("{file} = {format}"))
}

/// Patch `I386/DOSNET.INF`'s `[Files]` section by appending `d1,<file>`
/// entries for every slipstreamed file. DOSNET.INF controls which files
/// XP setup believes exist on the install source during the file-copy
/// phase; without these entries setup fails to find the slipstreamed
/// driver even when TXTSETUP.SIF correctly references it.
///
/// XP SP3's DOSNET.INF has multiple `[Files]` headers. We target the
/// LAST one (the bulk manifest with thousands of entries) -- adding to
/// the first/small header has not been validated and the canonical
/// MSFN / Tacktech examples all target the main file list.
///
/// `files` are the long-form filenames as they appear in the user's
/// TXTSETUP.OEM (case-preserving). DOSNET.INF entries in the wild are
/// conventionally lowercase; we follow that convention.
pub fn patch_dosnet_inf(text: &str, files: &[String]) -> Result<String> {
    if files.is_empty() {
        return Ok(text.to_string());
    }

    let lines: Vec<&str> = text.split("\r\n").collect();

    // Find LAST [Files] header.
    let mut last_files_header: Option<usize> = None;
    for (idx, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim_start();
        if let Some(name) = parse_section_header(trimmed) {
            if name.eq_ignore_ascii_case("Files") {
                last_files_header = Some(idx);
            }
        }
    }
    let header_idx =
        last_files_header.ok_or_else(|| anyhow::anyhow!("DOSNET.INF has no [Files] section"))?;

    // Walk forward from the header to find the end of this section: the
    // next section header or EOF. Insertion anchor is the last non-blank
    // line of the section body, so blank trailing lines are preserved
    // between our additions and the following section.
    let mut last_body_idx = header_idx;
    for (idx, raw) in lines.iter().enumerate().skip(header_idx + 1) {
        let trimmed = raw.trim_start();
        if parse_section_header(trimmed).is_some() {
            break;
        }
        if !raw.trim().is_empty() {
            last_body_idx = idx;
        }
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + files.len());
    for (idx, line) in lines.iter().enumerate() {
        out.push(line.to_string());
        if idx == last_body_idx {
            for file in files {
                out.push(format!("d1,{}", file.to_ascii_lowercase()));
            }
        }
    }
    Ok(out.join("\r\n"))
}

/// Insert lines into existing sections of an INI-shaped CRLF text.
///
/// Insertion point per section: directly after the last non-blank,
/// non-section-header line of the section's body. Trailing blanks
/// between the last body line and the next section header (if any) are
/// preserved.
///
/// Errors if a section named in `additions` is not present in `sif` --
/// the slipstream pipeline can't recover from a TXTSETUP.SIF missing
/// `[SCSI.Load]`, since the boot loader would refuse the unknown service.
fn apply_additions(sif: &str, additions: &SectionAdditions) -> Result<String> {
    let mut required: Vec<&'static str> = Vec::new();
    if !additions.scsi_load.is_empty() {
        required.push("SCSI.Load");
    }
    if !additions.scsi.is_empty() {
        required.push("SCSI");
    }
    if !additions.hardware_ids_database.is_empty() {
        required.push("HardwareIdsDatabase");
    }
    if !additions.source_disks_files.is_empty() {
        required.push("SourceDisksFiles");
    }

    // Walk lines, classify each, build out vector. Two stage:
    //   1. Identify the absolute line index of each section's "insertion
    //      anchor" (the index AFTER which we'll splice new lines).
    //   2. Walk again, emitting lines and inserting at the anchors.
    //
    // The first occurrence of each section name is the one we target;
    // a few sections (e.g. [SourceDisksFiles]) appear multiple times in
    // the XP SP3 file -- we add to the first one only, which is the
    // canonical full-file block at the top of the file.
    let mut lines: Vec<&str> = sif.split("\r\n").collect();
    // If the input didn't end with CRLF, split leaves a trailing empty
    // string; emit handles this naturally because join restores it.

    let mut current_section: Option<String> = None;
    let mut last_body_idx: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut section_first_seen: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    // Track sections we've already left (a different section header was
    // encountered after we entered them). If we re-enter such a section
    // later, we no longer treat it as the "first occurrence" -- body
    // updates from re-entries don't move our insertion anchor.
    //
    // XP SP3's TXTSETUP.SIF has FIVE separate `[SourceDisksFiles]`
    // headers; the FIRST one is the canonical full-file manifest
    // (~9000 lines), the others are smaller late blocks. Setup reads
    // from the first; if we add to the last, our entries are invisible.
    let mut section_left: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (idx, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim_start();
        if let Some(name) = parse_section_header(trimmed) {
            let key = name.to_ascii_lowercase();
            if let Some(prev) = &current_section {
                if prev != &key {
                    section_left.insert(prev.clone());
                }
            }
            current_section = Some(key.clone());
            section_first_seen.entry(key.clone()).or_insert(idx);
            continue;
        }
        if let Some(section) = &current_section {
            if section_left.contains(section) {
                // This is a body line inside a later occurrence of the
                // section. Don't update the anchor -- we want it pinned
                // to the first occurrence.
                continue;
            }
            let first_seen = section_first_seen[section];
            if first_seen <= idx && !raw.trim().is_empty() {
                last_body_idx.insert(section.clone(), idx);
            }
        }
    }

    for section in &required {
        let key = section.to_ascii_lowercase();
        if !section_first_seen.contains_key(&key) {
            bail!("TXTSETUP.SIF is missing required section [{}]", section);
        }
    }

    // Build a map: section first-header idx -> Vec of additions, keyed
    // by *anchor* (where we splice after).
    let mut splice_at: std::collections::HashMap<usize, Vec<String>> =
        std::collections::HashMap::new();
    for (key, header_idx) in &section_first_seen {
        if let Some(adds) = additions.for_section(key) {
            if adds.is_empty() {
                continue;
            }
            // Anchor: last body line of this section's first occurrence.
            // Fall back to the header line itself if the section is
            // empty (we'd insert directly after [SectionName]).
            let anchor = last_body_idx.get(key).copied().unwrap_or(*header_idx);
            splice_at
                .entry(anchor)
                .or_default()
                .extend(adds.iter().cloned());
        }
    }

    // Build output.
    let mut out: Vec<String> =
        Vec::with_capacity(lines.len() + splice_at.values().map(|v| v.len()).sum::<usize>());
    for (idx, line) in lines.drain(..).enumerate() {
        out.push(line.to_string());
        if let Some(adds) = splice_at.remove(&idx) {
            for a in adds {
                out.push(a);
            }
        }
    }

    Ok(out.join("\r\n"))
}

fn parse_section_header(trimmed: &str) -> Option<String> {
    if !trimmed.starts_with('[') {
        return None;
    }
    let close = trimmed.find(']')?;
    let name = trimmed[1..close].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_sif() -> String {
        // A condensed TXTSETUP.SIF skeleton with each of the four target
        // sections present and a couple of representative entries.
        let mut s = String::new();
        s.push_str("[Version]\r\n");
        s.push_str("Signature = \"$Windows NT$\"\r\n");
        s.push_str("\r\n");
        s.push_str("[SourceDisksFiles]\r\n");
        s.push_str("atapi.sys = 100,,,,,,4_,4,0,0,,1,4\r\n");
        s.push_str("intelide.sys = 100,,,,,,3_,4,1,,,1,4\r\n");
        s.push_str("\r\n");
        s.push_str("[HardwareIdsDatabase]\r\n");
        s.push_str("PCI\\VEN_8086&DEV_1230 = \"intelide\"\r\n");
        s.push_str("PCI\\VEN_8086&DEV_7010 = \"intelide\"\r\n");
        s.push_str("\r\n");
        s.push_str("[SCSI.Load]\r\n");
        s.push_str("atapi = atapi.sys,4\r\n");
        s.push_str("\r\n");
        s.push_str("[SCSI]\r\n");
        s.push_str("atapi = \"Standard IDE/ESDI Hard Disk Controller\"\r\n");
        s.push_str("\r\n");
        s
    }

    #[test]
    fn patch_inserts_scsi_load_entry_at_end_of_section() {
        let result = patch_txtsetup_sif(
            &sample_sif(),
            "iaStor",
            "Intel AHCI Controller",
            &["iaStor.sys".to_string()],
            &[(
                "PCI\\VEN_8086&DEV_3B2F&CC_0106".to_string(),
                "iaStor".to_string(),
            )],
        )
        .unwrap();
        // [SCSI.Load] gets one new line after the atapi line, before
        // the next section header.
        let load = result
            .text
            .split("[SCSI.Load]\r\n")
            .nth(1)
            .unwrap()
            .split("[")
            .next()
            .unwrap();
        assert!(load.contains("atapi = atapi.sys,4\r\n"));
        assert!(load.contains("iaStor = iaStor.sys,4\r\n"));
        // New line comes AFTER atapi.
        let atapi_off = load.find("atapi = atapi.sys").unwrap();
        let iastor_off = load.find("iaStor = iaStor.sys").unwrap();
        assert!(iastor_off > atapi_off);
    }

    #[test]
    fn patch_inserts_scsi_display_entry() {
        let result = patch_txtsetup_sif(
            &sample_sif(),
            "iaStor",
            "Intel AHCI Controller",
            &["iaStor.sys".to_string()],
            &[(
                "PCI\\VEN_8086&DEV_3B2F&CC_0106".to_string(),
                "iaStor".to_string(),
            )],
        )
        .unwrap();
        assert!(result
            .text
            .contains("iaStor = \"Intel AHCI Controller\"\r\n"));
    }

    #[test]
    fn patch_appends_every_pci_hwid_to_database() {
        let hwids = vec![
            (
                "PCI\\VEN_8086&DEV_3B2F&CC_0106".to_string(),
                "iaStor".to_string(),
            ),
            (
                "PCI\\VEN_8086&DEV_2929&CC_0106".to_string(),
                "iaStor".to_string(),
            ),
            (
                "PCI\\VEN_8086&DEV_282A&CC_0104".to_string(),
                "iaStor".to_string(),
            ),
        ];
        let result = patch_txtsetup_sif(
            &sample_sif(),
            "iaStor",
            "Intel AHCI Controller",
            &["iaStor.sys".to_string()],
            &hwids,
        )
        .unwrap();
        for (hwid, _) in &hwids {
            // LHS unquoted to match XP's own [HardwareIdsDatabase]
            // entries. Adding literal quote chars to the LHS makes XP's
            // parser compare against a HwID with embedded quote chars
            // and PnP silently fails to match.
            assert!(
                result.text.contains(&format!("{hwid} = \"iaStor\"\r\n")),
                "missing HwID {hwid}"
            );
            assert!(
                !result.text.contains(&format!("\"{hwid}\" = \"iaStor\"")),
                "HwID {hwid} should not be quoted on LHS"
            );
        }
        assert_eq!(result.additions.hardware_id_count, 3);
    }

    #[test]
    fn patch_skips_non_pci_hwids() {
        // FiraDisk's "detected\firadisk" pseudo id is not a PCI bus id
        // and shouldn't end up in [HardwareIdsDatabase] -- PnP only
        // enumerates PCI from there.
        let hwids = vec![
            (
                "PCI\\VEN_8086&DEV_3B2F&CC_0106".to_string(),
                "iaStor".to_string(),
            ),
            ("detected\\firadisk".to_string(), "FiraDisk".to_string()),
        ];
        let result = patch_txtsetup_sif(
            &sample_sif(),
            "iaStor",
            "Intel AHCI Controller",
            &["iaStor.sys".to_string()],
            &hwids,
        )
        .unwrap();
        assert!(result.text.contains("PCI\\VEN_8086&DEV_3B2F"));
        assert!(!result.text.contains("detected\\firadisk"));
        assert_eq!(result.additions.hardware_id_count, 1);
    }

    #[test]
    fn patch_adds_source_disks_files_lines_per_extension() {
        let result = patch_txtsetup_sif(
            &sample_sif(),
            "iaStor",
            "Intel AHCI Controller",
            &[
                "iaStor.sys".to_string(),
                "iaAHCI.inf".to_string(),
                "iaAHCI.cat".to_string(),
                "iaStor.inf".to_string(),
                "iaStor.cat".to_string(),
            ],
            &[(
                "PCI\\VEN_8086&DEV_3B2F&CC_0106".to_string(),
                "iaStor".to_string(),
            )],
        )
        .unwrap();
        // .sys uses field-1 `1` (base CD I386, where we append the
        // unpacked driver file) and field-7 `_x`
        // (uncompressed literal name). Trailing `,,,1,4` flags it as
        // boot-critical for ramdrive copy.
        assert!(result.text.contains("iaStor.sys = 1,,,,,,_x,4,1,,,1,4\r\n"));
        // .inf and .cat: same disk + _x, dest=20 (\inf), passive upgrade.
        assert!(result.text.contains("iaAHCI.inf = 1,,,,,,_x,20,0,0\r\n"));
        assert!(result.text.contains("iaAHCI.cat = 1,,,,,,_x,20,0,0\r\n"));
        assert!(result.text.contains("iaStor.inf = 1,,,,,,_x,20,0,0\r\n"));
        assert!(result.text.contains("iaStor.cat = 1,,,,,,_x,20,0,0\r\n"));
    }

    #[test]
    fn patch_preserves_existing_entries_verbatim() {
        let result = patch_txtsetup_sif(
            &sample_sif(),
            "iaStor",
            "Intel AHCI Controller",
            &["iaStor.sys".to_string()],
            &[(
                "PCI\\VEN_8086&DEV_3B2F&CC_0106".to_string(),
                "iaStor".to_string(),
            )],
        )
        .unwrap();
        // Every original line must still be present somewhere in output.
        for original in [
            "atapi.sys = 100,,,,,,4_,4,0,0,,1,4",
            "intelide.sys = 100,,,,,,3_,4,1,,,1,4",
            "PCI\\VEN_8086&DEV_1230 = \"intelide\"",
            "PCI\\VEN_8086&DEV_7010 = \"intelide\"",
            "atapi = atapi.sys,4",
            "atapi = \"Standard IDE/ESDI Hard Disk Controller\"",
            "Signature = \"$Windows NT$\"",
        ] {
            assert!(
                result.text.contains(original),
                "lost original line: {original}"
            );
        }
    }

    #[test]
    fn patch_fails_when_required_section_missing() {
        let mut sif = sample_sif();
        // Drop [SCSI.Load] entirely.
        let load_start = sif.find("[SCSI.Load]\r\n").unwrap();
        let load_end = sif[load_start..].find("\r\n\r\n").unwrap() + load_start + 4;
        sif.replace_range(load_start..load_end, "");
        let err = patch_txtsetup_sif(
            &sif,
            "iaStor",
            "Intel AHCI Controller",
            &["iaStor.sys".to_string()],
            &[(
                "PCI\\VEN_8086&DEV_3B2F&CC_0106".to_string(),
                "iaStor".to_string(),
            )],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("[SCSI.Load]"));
    }

    #[test]
    fn patch_fails_when_no_pci_hwids() {
        let err = patch_txtsetup_sif(
            &sample_sif(),
            "iaStor",
            "Intel AHCI Controller",
            &["iaStor.sys".to_string()],
            &[("detected\\firadisk".to_string(), "FiraDisk".to_string())],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no PCI entries"));
    }

    #[test]
    fn patch_fails_when_file_list_has_no_sys() {
        let err = patch_txtsetup_sif(
            &sample_sif(),
            "iaStor",
            "Intel AHCI Controller",
            &["iaStor.inf".to_string(), "iaStor.cat".to_string()],
            &[(
                "PCI\\VEN_8086&DEV_3B2F&CC_0106".to_string(),
                "iaStor".to_string(),
            )],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains(".sys driver"));
    }

    fn sample_dosnet() -> String {
        // DOSNET.INF has two [Files] sections in XP SP3: a tiny one
        // declaring usetup.exe + ntdll.dll boot files, and the big
        // file-copy manifest near the end. Mirror that shape here.
        let mut s = String::new();
        s.push_str("[Version]\r\n");
        s.push_str("Signature = \"$Windows NT$\"\r\n");
        s.push_str("\r\n");
        s.push_str("[Files]\r\n");
        s.push_str("d1,usetup.exe,system32\\smss.exe\r\n");
        s.push_str("d1,ntdll.dll,system32\\ntdll.dll\r\n");
        s.push_str("\r\n");
        s.push_str("[Strings]\r\n");
        s.push_str("foo = bar\r\n");
        s.push_str("\r\n");
        s.push_str("[Files]\r\n");
        s.push_str("d1,atapi.sys\r\n");
        s.push_str("d1,bootvid.dll\r\n");
        s.push_str("\r\n");
        s.push_str("[Trailing]\r\n");
        s.push_str("end\r\n");
        s
    }

    #[test]
    fn patch_dosnet_inf_appends_to_last_files_section() {
        let files = vec![
            "iaStor.sys".to_string(),
            "iaAHCI.inf".to_string(),
            "iaAHCI.cat".to_string(),
        ];
        let out = patch_dosnet_inf(&sample_dosnet(), &files).unwrap();
        // First [Files] section is untouched.
        let first_block = out
            .split("[Files]")
            .nth(1)
            .unwrap()
            .split('[')
            .next()
            .unwrap();
        assert!(first_block.contains("d1,usetup.exe"));
        assert!(!first_block.contains("d1,iastor.sys"));

        // Second [Files] section has the new entries appended, after
        // the existing atapi/bootvid lines, before [Trailing].
        let second_block = out
            .split("[Files]")
            .nth(2)
            .unwrap()
            .split('[')
            .next()
            .unwrap();
        assert!(second_block.contains("d1,atapi.sys"));
        assert!(second_block.contains("d1,bootvid.dll"));
        // Names are lowercased per DOSNET.INF convention.
        assert!(second_block.contains("d1,iastor.sys"));
        assert!(second_block.contains("d1,iaahci.inf"));
        assert!(second_block.contains("d1,iaahci.cat"));
        // Existing-then-new order: atapi precedes iastor in the section.
        let atapi_off = second_block.find("d1,atapi.sys").unwrap();
        let iastor_off = second_block.find("d1,iastor.sys").unwrap();
        assert!(atapi_off < iastor_off);
    }

    #[test]
    fn patch_dosnet_inf_preserves_other_sections_verbatim() {
        let out = patch_dosnet_inf(&sample_dosnet(), &["iaStor.sys".to_string()]).unwrap();
        for original in [
            "Signature = \"$Windows NT$\"",
            "d1,usetup.exe,system32\\smss.exe",
            "d1,ntdll.dll,system32\\ntdll.dll",
            "foo = bar",
            "d1,atapi.sys",
            "d1,bootvid.dll",
            "end",
        ] {
            assert!(out.contains(original), "lost original line: {original}");
        }
    }

    #[test]
    fn patch_dosnet_inf_no_op_for_empty_file_list() {
        let out = patch_dosnet_inf(&sample_dosnet(), &[]).unwrap();
        assert_eq!(out, sample_dosnet());
    }

    #[test]
    fn patch_dosnet_inf_fails_when_no_files_section() {
        let text = "[Version]\r\nSignature = \"x\"\r\n";
        let err = patch_dosnet_inf(text, &["iaStor.sys".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("[Files]"));
    }

    #[test]
    fn patch_real_xp_sp3_dosnet_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/xp_sp3/dosnet.inf");
        let dosnet = std::fs::read_to_string(&path).unwrap();
        let files = vec![
            "iaStor.sys".to_string(),
            "iaAHCI.inf".to_string(),
            "iaAHCI.cat".to_string(),
            "iaStor.inf".to_string(),
            "iaStor.cat".to_string(),
        ];
        let out = patch_dosnet_inf(&dosnet, &files).unwrap();
        for f in [
            "iastor.sys",
            "iaahci.inf",
            "iaahci.cat",
            "iastor.inf",
            "iastor.cat",
        ] {
            assert!(out.contains(&format!("d1,{f}")), "missing d1,{f}");
        }
        // Output grew exactly by the new lines (each + CRLF separator).
        assert!(out.len() > dosnet.len());
        // Original last line preserved.
        assert!(out.contains("[ForceCopyDriverCabFiles]"));
    }

    #[test]
    fn patch_targets_first_source_disks_files_when_multiple_present() {
        // XP SP3's TXTSETUP.SIF has FIVE separate [SourceDisksFiles]
        // headers. Setup reads from the FIRST one (the canonical full
        // manifest); the later trailing blocks are not the right place
        // to add slipstream entries. Verify our patcher targets the
        // first occurrence even when duplicates exist.
        let mut sif = String::new();
        sif.push_str("[Version]\r\nSignature = \"$Windows NT$\"\r\n\r\n");
        sif.push_str("[SourceDisksFiles]\r\n");
        sif.push_str("atapi.sys = 100,,,,,,4_,4,0,0,,1,4\r\n");
        sif.push_str("FIRST_BLOCK_LAST_LINE = 1\r\n");
        sif.push_str("\r\n");
        sif.push_str("[SourceDisksNames.x86]\r\n");
        sif.push_str("1 = foo\r\n");
        sif.push_str("\r\n");
        sif.push_str("[SourceDisksFiles]\r\n");
        sif.push_str("LATE_BLOCK_LINE = 1\r\n");
        sif.push_str("\r\n");
        sif.push_str("[HardwareIdsDatabase]\r\n");
        sif.push_str("PCI\\VEN_X = \"existing\"\r\n");
        sif.push_str("\r\n");
        sif.push_str("[SCSI.Load]\r\n");
        sif.push_str("atapi = atapi.sys,4\r\n");
        sif.push_str("\r\n");
        sif.push_str("[SCSI]\r\n");
        sif.push_str("atapi = \"ATAPI\"\r\n");
        sif.push_str("\r\n");

        let result = patch_txtsetup_sif(
            &sif,
            "iaStor",
            "Intel AHCI Controller",
            &["iaStor.sys".to_string()],
            &[(
                "PCI\\VEN_8086&DEV_3B2F&CC_0106".to_string(),
                "iaStor".to_string(),
            )],
        )
        .unwrap();

        // The new iaStor.sys line lands between the first block's last
        // entry and the next-section header -- NOT after LATE_BLOCK_LINE.
        let first_last = result.text.find("FIRST_BLOCK_LAST_LINE").unwrap();
        let iastor = result.text.find("iaStor.sys = ").unwrap();
        let late = result.text.find("LATE_BLOCK_LINE").unwrap();
        assert!(
            first_last < iastor,
            "iaStor.sys should be after first block's last entry"
        );
        assert!(
            iastor < late,
            "iaStor.sys should be BEFORE the second [SourceDisksFiles] block, not after"
        );
    }

    #[test]
    fn patch_real_xp_sp3_txtsetup_fixture() {
        // Hits the captured XP SP3 txtsetup.sif. Confirms the patcher
        // doesn't break on the real file shape (multiple
        // [SourceDisksFiles] blocks, very long [HardwareIdsDatabase],
        // etc) -- we only target the first occurrence of each section.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/xp_sp3/txtsetup.sif");
        let sif = std::fs::read_to_string(&path).unwrap();
        let result = patch_txtsetup_sif(
            &sif,
            "iaStor",
            "Intel AHCI Controller",
            &[
                "iaStor.sys".to_string(),
                "iaAHCI.inf".to_string(),
                "iaAHCI.cat".to_string(),
            ],
            &[(
                "PCI\\VEN_8086&DEV_3B2F&CC_0106".to_string(),
                "iaStor".to_string(),
            )],
        )
        .unwrap();
        // Every existing entry survives. The real fixture uses
        // lowercase `signature` and no spaces around `=`.
        assert!(result.text.contains("signature=\"$Windows NT$\""));
        assert!(result.text.contains("atapi = atapi.sys,4"));
        assert!(result
            .text
            .contains("PCI\\VEN_8086&DEV_1230 = \"intelide\""));
        // Our additions are present.
        assert!(result.text.contains("iaStor = iaStor.sys,4"));
        assert!(result.text.contains("iaStor = \"Intel AHCI Controller\""));
        assert!(result
            .text
            .contains("PCI\\VEN_8086&DEV_3B2F&CC_0106 = \"iaStor\""));
        assert!(result.text.contains("iaStor.sys = 1,,,,,,_x,4,1,,,1,4"));
        // File grew, not shrank.
        assert!(result.text.len() > sif.len());
    }
}
