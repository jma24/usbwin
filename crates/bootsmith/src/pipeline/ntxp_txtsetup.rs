//! Parser for `txtsetup.oem`-shaped INI files plus targeted manipulators.
//!
//! Originally built to merge a user-supplied F6 driver pack's
//! `txtsetup.oem` into the FiraDisk floppy's; that approach was retired
//! after hardware testing showed XP text-mode setup couldn't actually
//! auto-load multiple drivers from a single OEM floppy. The slipstream
//! pipeline replaced it. The merge / strip / scsi-controllers helpers
//! remain because the slipstream code needs to parse the user's
//! txtsetup.oem (to enumerate controllers, pull HwIDs, and resolve
//! driver filenames). Unused-warning suppression below covers helpers
//! that are still useful primitives but currently dormant.
#![allow(dead_code)]

use anyhow::{bail, Context, Result};

/// One section of a txtsetup.oem-shaped INI file. `name` is the bracketed
/// header (without brackets); `lines` is the body verbatim. The header itself
/// is *not* included in `lines`.
#[derive(Debug, Clone)]
struct Section {
    name: String,
    lines: Vec<String>,
}

/// Merge `oem` into `base`. Returns the combined txtsetup.oem text suitable
/// for replacing the FiraDisk floppy's copy.
///
/// Both files describe drivers that live on the SAME physical FiraDisk
/// floppy (the OEM pack's files get copied into the floppy root alongside
/// FiraDisk's own). So colliding disk IDs (e.g. both files declaring
/// `disk1`) are not separate physical media -- they're the same floppy.
/// We collapse: keep the base's `[Disks]` entry, drop the OEM's, and
/// leave OEM `[Files.scsi.X]` references pointing at the same disk ID
/// (which now resolves to the base's entry).
///
/// This matters for the auto-load (`[MassStorageDrivers]` +
/// `OemPreinstall = Yes`) path -- XP validates every `[Disks]` entry as
/// a separate OEM source, and when two entries point at one floppy it
/// bails at `oemdisk.c:1747` (error 18). The manual F6 flow only loads
/// from one disk at a time and never tripped this.
pub fn merge(base: &str, oem: &str) -> Result<String> {
    let base_sections = parse(base).context("parse base txtsetup.oem (FiraDisk)")?;
    let oem_sections = parse(oem).context("parse user txtsetup.oem")?;
    Ok(emit(combine(base_sections, oem_sections)))
}

fn parse(text: &str) -> Result<Vec<Section>> {
    let mut sections = Vec::new();
    let mut current: Option<Section> = None;
    for (lineno, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with('[') {
            let close = trimmed
                .find(']')
                .ok_or_else(|| anyhow::anyhow!("unterminated section header at line {lineno}"))?;
            let name = trimmed[1..close].trim().to_string();
            if name.is_empty() {
                bail!("empty section header at line {lineno}");
            }
            if let Some(prev) = current.take() {
                sections.push(prev);
            }
            current = Some(Section {
                name,
                lines: Vec::new(),
            });
        } else if let Some(sec) = current.as_mut() {
            sec.lines.push(raw.to_string());
        } else if !trimmed.is_empty() && !trimmed.starts_with(';') {
            // Stray non-comment content before any section -- carry as a
            // synthetic header so we don't drop it on emit.
            current = Some(Section {
                name: "__preamble".to_string(),
                lines: vec![raw.to_string()],
            });
        }
    }
    if let Some(prev) = current.take() {
        sections.push(prev);
    }
    Ok(sections)
}

fn emit(sections: Vec<Section>) -> String {
    let mut out = String::new();
    for sec in sections {
        if sec.name == "__preamble" {
            for line in sec.lines {
                out.push_str(&line);
                out.push_str("\r\n");
            }
            continue;
        }
        out.push('[');
        out.push_str(&sec.name);
        out.push_str("]\r\n");
        for line in sec.lines {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    out
}

fn parse_key(line: &str) -> Option<String> {
    let body = strip_comment(line);
    let eq = body.find('=')?;
    let key = body[..eq].trim();
    if key.is_empty() {
        return None;
    }
    Some(key.to_string())
}

fn strip_comment(line: &str) -> &str {
    // txtsetup.oem uses `;` for comments. The OEM lines never put `;` in
    // string values that we care about, so a plain split is safe.
    match line.find(';') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn combine(base: Vec<Section>, oem: Vec<Section>) -> Vec<Section> {
    // Sections we coalesce (case-insensitive name match) by appending the
    // OEM body after the base body.
    const COALESCE: &[&str] = &["Disks", "SCSI"];

    let mut out = base;

    for sec in oem {
        if sec.name.eq_ignore_ascii_case("Defaults") || sec.name == "__preamble" {
            // FiraDisk's default mass-storage driver wins; the preamble
            // comment block is also redundant.
            continue;
        }
        if COALESCE
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&sec.name))
        {
            if let Some(existing) = out
                .iter_mut()
                .find(|s| s.name.eq_ignore_ascii_case(&sec.name))
            {
                if sec.name.eq_ignore_ascii_case("Disks") {
                    // Both txtsetup.oem inputs describe drivers on the
                    // same physical floppy. Drop OEM disk lines whose ID
                    // already exists in base -- otherwise XP under
                    // OemPreinstall sees them as duplicate physical media.
                    let existing_ids: Vec<String> = existing
                        .lines
                        .iter()
                        .filter_map(|l| parse_key(l))
                        .map(|k| k.to_ascii_lowercase())
                        .collect();
                    for line in sec.lines {
                        match parse_key(&line) {
                            Some(id)
                                if existing_ids.contains(&id.to_ascii_lowercase()) =>
                            {
                                // Drop -- already declared by base.
                            }
                            _ => existing.lines.push(line),
                        }
                    }
                } else {
                    existing.lines.extend(sec.lines);
                }
                continue;
            }
        }
        out.push(sec);
    }
    out
}

/// Plug-and-play hardware IDs declared by `[HardwareIds.scsi.<id>]`
/// sections, paired with the service name each one maps to.
///
/// Returned tuples are `(hardware_id, service_name)`, e.g.
/// `("PCI\\VEN_8086&DEV_3B2F&CC_0106", "iaStor")`. The slipstream
/// pipeline feeds these into the staged ISO's `I386\TXTSETUP.SIF`
/// `[HardwareIdsDatabase]` so XP text-mode setup PnP can match the
/// driver by PCI ID without any user F6 action. Duplicates (same id,
/// same service) are coalesced; conflicting service mappings for the
/// same id are an error.
pub fn hardware_ids(text: &str) -> Result<Vec<(String, String)>> {
    let sections = parse(text).context("parse txtsetup.oem for hardware ids")?;
    let mut out: Vec<(String, String)> = Vec::new();
    for sec in &sections {
        if !sec
            .name
            .to_ascii_lowercase()
            .starts_with("hardwareids.scsi.")
        {
            continue;
        }
        for line in &sec.lines {
            let body = strip_comment(line);
            let Some(eq) = body.find('=') else { continue };
            // Form: id = "PCI\...","service"  -- the value is a comma-
            // separated pair of quoted strings.
            let rhs = body[eq + 1..].trim();
            let mut depth = 0usize;
            let mut split: Option<usize> = None;
            for (i, ch) in rhs.char_indices() {
                match ch {
                    '"' => depth ^= 1,
                    ',' if depth == 0 => {
                        split = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let (hwid_raw, service_raw) = match split {
                Some(i) => (rhs[..i].trim(), rhs[i + 1..].trim()),
                None => continue,
            };
            let hwid = strip_quotes(hwid_raw).to_string();
            let service = strip_quotes(service_raw).to_string();
            if hwid.is_empty() || service.is_empty() {
                continue;
            }
            if let Some((_, existing_service)) =
                out.iter().find(|(h, _)| h.eq_ignore_ascii_case(&hwid))
            {
                if !existing_service.eq_ignore_ascii_case(&service) {
                    bail!(
                        "TXTSETUP.OEM declares hardware id {} for two services ({} vs {})",
                        hwid,
                        existing_service,
                        service
                    );
                }
                continue;
            }
            out.push((hwid, service));
        }
    }
    Ok(out)
}

/// Filenames referenced by the per-driver `[Files.scsi.<id>]` sections of a
/// txtsetup.oem. The pipeline uses this list to decide which files in the
/// user's --ahci-driver-dir actually need to be copied onto the floppy.
pub fn referenced_filenames(text: &str) -> Result<Vec<String>> {
    let sections = parse(text).context("parse txtsetup.oem for file references")?;
    let mut out = Vec::new();
    for sec in &sections {
        if !sec.name.to_ascii_lowercase().starts_with("files.scsi.") {
            continue;
        }
        for line in &sec.lines {
            let body = strip_comment(line);
            let Some(eq) = body.find('=') else { continue };
            let rhs = &body[eq + 1..];
            // Form: "diskN, filename, [optional service]".
            let mut fields = rhs.split(',');
            let _disk = fields.next();
            if let Some(file) = fields.next() {
                let name = file.trim();
                if !name.is_empty() && !out.iter().any(|s: &String| s.eq_ignore_ascii_case(name)) {
                    out.push(name.to_string());
                }
            }
        }
    }
    Ok(out)
}

/// One row from the `[scsi]` section joined with the file references in
/// the corresponding `[Files.scsi.<id>]` section.
#[derive(Debug, Clone)]
pub struct ScsiController {
    pub id: String,
    pub display_name: String,
    pub driver: Option<String>,
    pub inf: Option<String>,
    pub catalog: Option<String>,
}

impl ScsiController {
    pub fn referenced_files(&self) -> impl Iterator<Item = &str> {
        [self.driver.as_deref(), self.inf.as_deref(), self.catalog.as_deref()]
            .into_iter()
            .flatten()
    }
}

/// Drop one or more `[scsi]` entries (by id) plus their associated
/// `[Files.scsi.<id>]` and `[HardwareIds.scsi.<id>]` sections from a
/// txtsetup.oem text. The pipeline uses this to scrub the embedded
/// FiraDisk floppy's `firadiskx64` declaration (and any other dangling
/// references in a user-supplied OEM pack) before writing TXTSETUP.OEM
/// back to the floppy -- XP under `OemPreinstall = Yes` validates every
/// `[Files.scsi.*]` block in the file, not just the ones named in
/// `[MassStorageDrivers]`, and bails at `oemdisk.c:1747` (error 18) on
/// the first missing referenced file.
///
/// Matching is case-insensitive. Unknown ids are silently ignored.
pub fn strip_controllers(text: &str, ids_to_drop: &[String]) -> Result<String> {
    if ids_to_drop.is_empty() {
        return Ok(text.to_string());
    }
    let mut sections = parse(text).context("parse txtsetup.oem for filter")?;

    let drop_id = |id: &str| -> bool {
        ids_to_drop
            .iter()
            .any(|d| d.eq_ignore_ascii_case(id))
    };

    // 1) Remove per-controller sections wholesale.
    sections.retain(|sec| {
        let lname = sec.name.to_ascii_lowercase();
        let suffix = lname
            .strip_prefix("files.scsi.")
            .or_else(|| lname.strip_prefix("hardwareids.scsi."));
        match suffix {
            Some(id) => !drop_id(id),
            None => true,
        }
    });

    // 2) Strip matching `id = ...` lines from [scsi]/[SCSI] section bodies.
    for sec in sections.iter_mut() {
        if !sec.name.eq_ignore_ascii_case("SCSI") {
            continue;
        }
        sec.lines.retain(|line| {
            let body = strip_comment(line);
            match body.find('=') {
                Some(eq) => {
                    let key = body[..eq].trim();
                    !drop_id(key)
                }
                None => true,
            }
        });
    }

    Ok(emit(sections))
}

/// Parse a txtsetup.oem and return one `ScsiController` per `[scsi]` entry,
/// joined with the driver/inf/catalog filenames from its
/// `[Files.scsi.<id>]` section. Pipeline uses this to filter the auto-load
/// `[MassStorageDrivers]` list down to controllers whose driver files
/// actually exist on the FiraDisk floppy.
pub fn scsi_controllers(text: &str) -> Result<Vec<ScsiController>> {
    let sections = parse(text).context("parse txtsetup.oem for scsi controllers")?;

    // First pass: collect (id, display_name) from [scsi]/[SCSI].
    let mut controllers: Vec<ScsiController> = Vec::new();
    for sec in &sections {
        if !sec.name.eq_ignore_ascii_case("SCSI") {
            continue;
        }
        for line in &sec.lines {
            let body = strip_comment(line);
            let Some(eq) = body.find('=') else { continue };
            let id = body[..eq].trim();
            if id.is_empty() {
                continue;
            }
            let rhs = body[eq + 1..].trim();
            let display = strip_quotes(rhs);
            if display.is_empty() {
                continue;
            }
            if controllers.iter().any(|c| c.id.eq_ignore_ascii_case(id)) {
                continue;
            }
            controllers.push(ScsiController {
                id: id.to_string(),
                display_name: display.to_string(),
                driver: None,
                inf: None,
                catalog: None,
            });
        }
    }

    // Second pass: pick up driver/inf/catalog filenames from
    // [Files.scsi.<id>] sections.
    for sec in &sections {
        let lname = sec.name.to_ascii_lowercase();
        let Some(id) = lname.strip_prefix("files.scsi.") else {
            continue;
        };
        let Some(c) = controllers
            .iter_mut()
            .find(|c| c.id.eq_ignore_ascii_case(id))
        else {
            continue;
        };
        for line in &sec.lines {
            let body = strip_comment(line);
            let Some(eq) = body.find('=') else { continue };
            let key = body[..eq].trim().to_ascii_lowercase();
            let rhs = &body[eq + 1..];
            // Form: "diskN, filename[, optional service]". Take field 1.
            let mut fields = rhs.split(',');
            let _disk = fields.next();
            let Some(filename) = fields.next() else { continue };
            let filename = filename.trim();
            if filename.is_empty() {
                continue;
            }
            match key.as_str() {
                "driver" => c.driver = Some(filename.to_string()),
                "inf" => c.inf = Some(filename.to_string()),
                "catalog" => c.catalog = Some(filename.to_string()),
                _ => {}
            }
        }
    }

    Ok(controllers)
}

fn strip_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRADISK: &str = "[Disks]\r\n\
disk1=\"FiraDisk Installation Disk\",\\firadisk.inf,\\\r\n\
\r\n\
[Defaults]\r\n\
SCSI=firadiskx86\r\n\
\r\n\
[SCSI]\r\n\
firadiskx86=\"FiraDisk Driver x86\"\r\n\
\r\n\
[Files.scsi.firadiskx86]\r\n\
driver=disk1,firadisk.sys,FiraDisk\r\n\
inf=disk1,firadisk.inf\r\n\
catalog=disk1,firadisk.cat\r\n\
\r\n\
[HardwareIds.scsi.firadiskx86]\r\n\
id=\"detected\\firadisk\",\"FiraDisk\"\r\n";

    const IASTOR: &str = "[Disks]\r\n\
disk1 = \"Intel(R) Rapid Storage Technology Driver\", iaStor.sys, \\\r\n\
\r\n\
[Defaults]\r\n\
scsi = iaStor_8ME9ME5\r\n\
\r\n\
[scsi]\r\n\
iaAHCI_9MEM = \"Intel(R) ICH9M-E/M SATA AHCI Controller\"\r\n\
iaStor_8ME9ME5 = \"Intel(R) ICH8M-E/ICH9M-E/5 Series SATA RAID Controller\"\r\n\
\r\n\
[Files.scsi.iaAHCI_9MEM]\r\n\
driver = disk1, iaStor.sys, iaStor\r\n\
inf = disk1, iaAHCI.inf\r\n\
catalog = disk1, iaAHCI.cat\r\n\
\r\n\
[Files.scsi.iaStor_8ME9ME5]\r\n\
driver = disk1, iaStor.sys, iaStor\r\n\
inf = disk1, iaStor.inf\r\n\
catalog = disk1, iaStor.cat\r\n\
\r\n\
[HardwareIds.scsi.iaAHCI_9MEM]\r\n\
id = \"PCI\\VEN_8086&DEV_2929&CC_0106\",\"iaStor\"\r\n";

    #[test]
    fn merge_collapses_colliding_disk_id_to_single_entry() {
        // Both files declare disk1 = ..., but in practice both describe
        // drivers on the same physical FiraDisk floppy. The merge keeps
        // base's (FiraDisk's) disk1 entry and drops OEM's; all
        // [Files.scsi.X] references stay as disk1, which now resolves to
        // the single base entry. XP under OemPreinstall=Yes can't handle
        // two [Disks] entries pointing at one floppy, so collapsing is
        // required for the auto-load path to parse.
        let merged = merge(FIRADISK, IASTOR).unwrap();

        // FiraDisk's disk1 is the only [Disks] entry.
        let disks_blocks: Vec<&str> = merged.split("[Disks]").collect();
        assert_eq!(disks_blocks.len(), 2, "expected exactly one [Disks] header");
        let disks_body = disks_blocks[1].split('[').next().unwrap_or("");
        assert!(disks_body.contains("disk1=\"FiraDisk Installation Disk\""));
        assert!(
            !disks_body.contains("Intel(R) Rapid Storage Technology Driver"),
            "OEM's [Disks] entry should have been dropped, body was:\n{disks_body}"
        );
        // No disk2 anywhere (no rename happened).
        assert!(!merged.contains("disk2"));

        // OEM's [Files.scsi.X] references stay as disk1 (which now
        // resolves to FiraDisk's collapsed entry).
        let ia_block = merged
            .split("[Files.scsi.iaAHCI_9MEM]")
            .nth(1)
            .expect("iaAHCI section present");
        assert!(ia_block.contains("driver = disk1"));
        assert!(ia_block.contains("inf = disk1"));
        assert!(ia_block.contains("catalog = disk1"));
        // FiraDisk's references unchanged.
        let fd_block = merged
            .split("[Files.scsi.firadiskx86]")
            .nth(1)
            .expect("firadisk section present");
        assert!(fd_block.contains("driver=disk1"));
    }

    #[test]
    fn merge_keeps_firadisk_default_and_drops_oem_default() {
        let merged = merge(FIRADISK, IASTOR).unwrap();
        assert!(merged.contains("SCSI=firadiskx86"));
        assert!(!merged.contains("scsi = iaStor_8ME9ME5"));
    }

    #[test]
    fn merge_coalesces_scsi_section() {
        let merged = merge(FIRADISK, IASTOR).unwrap();
        // Both SCSI sections should be coalesced into the first occurrence
        // (case-insensitive). The base section is named "SCSI"; the OEM is
        // "scsi". They merge under the base's name and casing.
        let scsi_blocks: Vec<&str> = merged.split("[SCSI]").collect();
        assert_eq!(scsi_blocks.len(), 2, "expected exactly one [SCSI] header");
        let scsi = scsi_blocks[1];
        assert!(scsi.contains("firadiskx86=\"FiraDisk Driver x86\""));
        assert!(scsi.contains("iaAHCI_9MEM = \"Intel(R) ICH9M-E/M SATA AHCI Controller\""));
        assert!(scsi.contains(
            "iaStor_8ME9ME5 = \"Intel(R) ICH8M-E/ICH9M-E/5 Series SATA RAID Controller\""
        ));
        // [scsi] header should not also appear (would mean we failed to coalesce).
        assert!(!merged.contains("[scsi]"));
    }

    #[test]
    fn merge_coalesces_disks_into_single_header() {
        let merged = merge(FIRADISK, IASTOR).unwrap();
        let disks_blocks: Vec<&str> = merged.split("[Disks]").collect();
        assert_eq!(disks_blocks.len(), 2, "expected exactly one [Disks] header");
        // After collapse there should be only one disk line (FiraDisk's).
        // OEM's disk1 collided with base's disk1 and was dropped.
        let disks_body = disks_blocks[1].split('[').next().unwrap_or("");
        let disk_lines: Vec<&str> = disks_body
            .lines()
            .filter(|l| l.trim_start().starts_with("disk"))
            .collect();
        assert_eq!(
            disk_lines.len(),
            1,
            "expected one collapsed disk entry, got:\n{disks_body}"
        );
        assert!(disk_lines[0].contains("disk1"));
        assert!(disk_lines[0].contains("FiraDisk"));
    }

    #[test]
    fn merge_keeps_non_colliding_oem_disks() {
        // Sanity: if an OEM pack declares a unique disk ID (rare, since
        // every Intel/AMD pack uses disk1), it should survive the
        // coalesce and join the base's entry rather than being dropped.
        let oem = "[Disks]\r\ndiskExtra=\"Some Other Disk\", extra.sys, \\\r\n";
        let merged = merge(FIRADISK, oem).unwrap();
        let disks_body = merged.split("[Disks]").nth(1).unwrap();
        let disks_body = disks_body.split('[').next().unwrap();
        assert!(disks_body.contains("disk1=\"FiraDisk"));
        assert!(disks_body.contains("diskExtra=\"Some Other Disk\""));
    }

    #[test]
    fn merge_is_idempotent_for_non_colliding_input() {
        let base = "[Disks]\r\ndisk1=\"A\",\\a.inf,\\\r\n";
        let oem = "[Disks]\r\ndiskAlpha=\"B\",b.sys,\\\r\n";
        let merged = merge(base, oem).unwrap();
        assert!(merged.contains("disk1=\"A\""));
        assert!(merged.contains("diskAlpha=\"B\""));
        // No rename should have happened.
        assert!(!merged.contains("disk2"));
    }

    #[test]
    fn referenced_filenames_returns_unique_files() {
        let files = referenced_filenames(IASTOR).unwrap();
        // iaStor.sys appears in both driver blocks but should be deduped.
        assert_eq!(files.iter().filter(|f| f.eq_ignore_ascii_case("iaStor.sys")).count(), 1);
        assert!(files.iter().any(|f| f.eq_ignore_ascii_case("iaAHCI.inf")));
        assert!(files.iter().any(|f| f.eq_ignore_ascii_case("iaAHCI.cat")));
        assert!(files.iter().any(|f| f.eq_ignore_ascii_case("iaStor.inf")));
        assert!(files.iter().any(|f| f.eq_ignore_ascii_case("iaStor.cat")));
    }

    #[test]
    fn referenced_filenames_handles_firadisk_blob() {
        let files = referenced_filenames(FIRADISK).unwrap();
        assert!(files.iter().any(|f| f.eq_ignore_ascii_case("firadisk.sys")));
        assert!(files.iter().any(|f| f.eq_ignore_ascii_case("firadisk.inf")));
        assert!(files.iter().any(|f| f.eq_ignore_ascii_case("firadisk.cat")));
    }

    #[test]
    fn hardware_ids_parse_intel_ahci_block() {
        let ids = hardware_ids(IASTOR).unwrap();
        assert_eq!(
            ids,
            vec![(
                "PCI\\VEN_8086&DEV_2929&CC_0106".to_string(),
                "iaStor".to_string()
            )]
        );
    }

    #[test]
    fn hardware_ids_dedup_repeated_pairs() {
        // Multi-line block where the same id is repeated -- should
        // collapse silently.
        let text = "[HardwareIds.scsi.foo]\r\n\
id = \"PCI\\VEN_8086&DEV_2929\",\"iaStor\"\r\n\
id = \"PCI\\VEN_8086&DEV_2929\",\"iaStor\"\r\n";
        let ids = hardware_ids(text).unwrap();
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn hardware_ids_reject_conflicting_service_for_same_id() {
        let text = "[HardwareIds.scsi.foo]\r\n\
id = \"PCI\\VEN_8086&DEV_2929\",\"iaStor\"\r\n\
\r\n\
[HardwareIds.scsi.bar]\r\n\
id = \"PCI\\VEN_8086&DEV_2929\",\"otherDriver\"\r\n";
        let err = hardware_ids(text).unwrap_err().to_string();
        assert!(err.contains("two services"));
    }

    #[test]
    fn hardware_ids_walk_every_block_in_merged_oem() {
        // Sanity: scsi_controllers covers the [HardwareIds.scsi.*]
        // sections of a full Intel pack; we should pull every PCI id
        // out, one per AHCI controller block.
        let oem = "[HardwareIds.scsi.A]\r\nid = \"PCI\\VEN_8086&DEV_AAAA\",\"iaStor\"\r\n\r\n\
[HardwareIds.scsi.B]\r\nid = \"PCI\\VEN_8086&DEV_BBBB\",\"iaStor\"\r\n";
        let ids = hardware_ids(oem).unwrap();
        let hwids: Vec<&str> = ids.iter().map(|(h, _)| h.as_str()).collect();
        assert!(hwids.contains(&"PCI\\VEN_8086&DEV_AAAA"));
        assert!(hwids.contains(&"PCI\\VEN_8086&DEV_BBBB"));
    }

    #[test]
    fn hardware_ids_skip_pseudo_detected_ids() {
        // FiraDisk uses a fake "detected\firadisk" entry that PnP
        // doesn't actually enumerate. We still surface it -- callers
        // can filter on the PCI\ prefix if they care; the parser is
        // format-faithful.
        let ids = hardware_ids(FIRADISK).unwrap();
        assert!(ids
            .iter()
            .any(|(h, _)| h.eq_ignore_ascii_case("detected\\firadisk")));
    }

    #[test]
    fn merge_against_full_intel_oem_keeps_all_controller_ids() {
        // Sanity: when we merge a more realistic OEM body, none of the
        // OEM-listed controller IDs disappear.
        let oem = "[Disks]\r\n\
disk1 = \"Intel\", iaStor.sys, \\\r\n\
\r\n\
[scsi]\r\n\
iaAHCI_ESB2 = \"x\"\r\n\
iaAHCI_9MEM = \"y\"\r\n\
iaStor_8ME9ME5 = \"z\"\r\n\
\r\n\
[Files.scsi.iaAHCI_ESB2]\r\n\
driver = disk1, iaStor.sys, iaStor\r\n\
inf = disk1, iaAHCI.inf\r\n\
catalog = disk1, iaAHCI.cat\r\n";
        let merged = merge(FIRADISK, oem).unwrap();
        for id in ["iaAHCI_ESB2", "iaAHCI_9MEM", "iaStor_8ME9ME5"] {
            assert!(
                merged.contains(id),
                "merged file is missing controller id {id}"
            );
        }
    }

    #[test]
    fn scsi_controllers_strip_quotes_and_handle_comments() {
        let text = "[SCSI]\r\n\
; commented out\r\n\
foo = \"Foo Controller\"\r\n\
bar = PlainName\r\n\
\r\n\
baz = \"Baz\" ; trailing comment\r\n";
        let cs = scsi_controllers(text).unwrap();
        let names: Vec<_> = cs.iter().map(|c| c.display_name.as_str()).collect();
        assert_eq!(names, vec!["Foo Controller", "PlainName", "Baz"]);
    }

    #[test]
    fn scsi_controllers_dedup_by_id() {
        let text = "[SCSI]\r\nfoo = \"Foo\"\r\nfoo = \"Foo Again\"\r\n";
        let cs = scsi_controllers(text).unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].display_name, "Foo");
    }

    #[test]
    fn scsi_controllers_ignore_files_scsi_sections_as_scsi_blocks() {
        // A [Files.scsi.X] section also starts with "scsi" but must not
        // be confused with the controller [scsi] section.
        let text = "[SCSI]\r\n\
foo = \"Foo\"\r\n\
\r\n\
[Files.scsi.foo]\r\n\
driver = disk1, foo.sys, foo\r\n";
        let cs = scsi_controllers(text).unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].display_name, "Foo");
    }

    #[test]
    fn scsi_controllers_pull_firadisk_plus_intel_from_merge() {
        // Feed in the merged OEM, get every controller (FiraDisk + Intel)
        // so the auto-load pipeline can list them all in
        // [MassStorageDrivers] for HwID-driven binding.
        let merged = merge(FIRADISK, IASTOR).unwrap();
        let cs = scsi_controllers(&merged).unwrap();
        let names: Vec<_> = cs.iter().map(|c| c.display_name.as_str()).collect();
        assert!(names.contains(&"FiraDisk Driver x86"));
        assert!(names.contains(&"Intel(R) ICH9M-E/M SATA AHCI Controller"));
        assert!(names.contains(&"Intel(R) ICH8M-E/ICH9M-E/5 Series SATA RAID Controller"));
    }

    #[test]
    fn scsi_controllers_join_display_name_with_file_references() {
        let merged = merge(FIRADISK, IASTOR).unwrap();
        let cs = scsi_controllers(&merged).unwrap();
        let by_name = |n: &str| {
            cs.iter()
                .find(|c| c.display_name == n)
                .unwrap_or_else(|| panic!("missing controller {n}"))
                .clone()
        };
        let fd = by_name("FiraDisk Driver x86");
        assert_eq!(fd.driver.as_deref(), Some("firadisk.sys"));
        assert_eq!(fd.inf.as_deref(), Some("firadisk.inf"));
        assert_eq!(fd.catalog.as_deref(), Some("firadisk.cat"));

        let ich9 = by_name("Intel(R) ICH9M-E/M SATA AHCI Controller");
        assert_eq!(ich9.driver.as_deref(), Some("iaStor.sys"));
        assert_eq!(ich9.inf.as_deref(), Some("iaAHCI.inf"));
        assert_eq!(ich9.catalog.as_deref(), Some("iaAHCI.cat"));
    }

    #[test]
    fn strip_controllers_removes_scsi_files_and_hardwareids_for_id() {
        // This is the embedded-FiraDisk-x64 case verbatim: declared in
        // [scsi], with its own [Files.scsi.X] and [HardwareIds.scsi.X]
        // sections, but the binaries aren't shipped. Strip it.
        let text = "[Disks]\r\n\
disk1=\"FiraDisk Installation Disk\",\\firadisk.inf,\\\r\n\
\r\n\
[SCSI]\r\n\
firadiskx86=\"FiraDisk Driver x86\"\r\n\
firadiskx64=\"FiraDisk Driver x64\"\r\n\
\r\n\
[Files.scsi.firadiskx86]\r\n\
driver=disk1,firadisk.sys,FiraDisk\r\n\
inf=disk1,firadisk.inf\r\n\
catalog=disk1,firadisk.cat\r\n\
\r\n\
[Files.scsi.firadiskx64]\r\n\
driver=disk1,firadi64.sys,FiraDisk\r\n\
inf=disk1,firadisk.inf\r\n\
catalog=disk1,firadi64.cat\r\n\
\r\n\
[HardwareIds.scsi.firadiskx86]\r\n\
id=\"detected\\firadisk\",\"FiraDisk\"\r\n\
\r\n\
[HardwareIds.scsi.firadiskx64]\r\n\
id=\"detected\\firadisk\",\"FiraDisk\"\r\n";
        let out = strip_controllers(text, &["firadiskx64".to_string()]).unwrap();
        assert!(out.contains("firadiskx86"));
        assert!(!out.contains("firadiskx64"));
        assert!(out.contains("[Files.scsi.firadiskx86]"));
        assert!(!out.contains("[Files.scsi.firadiskx64]"));
        assert!(out.contains("[HardwareIds.scsi.firadiskx86]"));
        assert!(!out.contains("[HardwareIds.scsi.firadiskx64]"));
        assert!(!out.contains("firadi64.sys"));
        assert!(!out.contains("firadi64.cat"));
    }

    #[test]
    fn strip_controllers_is_case_insensitive() {
        let text = "[SCSI]\r\nFooBar = \"Foo\"\r\n\r\n[Files.scsi.FOOBAR]\r\ndriver=disk1,a.sys,a\r\n";
        let out = strip_controllers(text, &["foobar".to_string()]).unwrap();
        assert!(!out.contains("FooBar"));
        assert!(!out.contains("[Files.scsi.FOOBAR]"));
    }

    #[test]
    fn strip_controllers_no_op_for_empty_drop_list() {
        let text = "[SCSI]\r\nfoo = \"Foo\"\r\n";
        let out = strip_controllers(text, &[]).unwrap();
        assert_eq!(out, text);
    }

    #[test]
    fn scsi_controllers_leave_files_none_when_section_missing() {
        // No [Files.scsi.foo] section -> the controller is still parsed
        // but with no associated files. Callers should treat that as
        // "skip this entry" since text-mode setup needs the .sys to bind.
        let text = "[SCSI]\r\nfoo = \"Foo Controller\"\r\n";
        let cs = scsi_controllers(text).unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].display_name, "Foo Controller");
        assert!(cs[0].driver.is_none());
        assert!(cs[0].inf.is_none());
        assert!(cs[0].catalog.is_none());
    }
}
