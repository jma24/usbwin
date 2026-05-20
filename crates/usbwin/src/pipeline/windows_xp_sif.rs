//! `txtsetup.sif` parser + modifier for the WinSetupFromUSB-style XP install
//! recipe.
//!
//! The file is an INI-flavoured format:
//!   - Sections: `[Name]` on a line by itself.
//!   - Entries: `key = value` (whitespace around `=` varies).
//!   - Comments: `;` to end of line.
//!   - Blank lines preserved for readability.
//!
//! XP SP3's txtsetup.sif is ~22000 lines of CRLF-terminated ASCII. The
//! modifier preserves the file byte-for-byte except for the specific lines
//! it moves or adds. Format-preserving fidelity matters: the file is read
//! by NTLDR/setupldr in text mode, and minor formatting differences can
//! cause obscure setup failures.

use std::collections::BTreeMap;

/// Parsed view of a SIF file as a sequence of lines that remembers which
/// section each line belongs to. `lines` keeps the original strings (sans
/// trailing CRLF/LF, which we recover at render time).
#[derive(Debug, Clone)]
pub struct Sif {
    /// All lines in original order. We don't strip whitespace, comments,
    /// or anything else — modifications are line moves and line additions
    /// only.
    pub lines: Vec<String>,
    /// Map from section name (without brackets) to the line indices that
    /// belong to that section's body — i.e. lines AFTER the `[Name]` header
    /// up to but not including the next `[…]` header.
    pub sections: BTreeMap<String, SectionRange>,
    /// Whether the original file used CRLF (true) or LF (false) endings.
    /// XP txtsetup.sif uses CRLF; preserve that on render.
    pub uses_crlf: bool,
}

/// A section's line range. `header` is the index of the `[Name]` line.
/// `body_start` is `header + 1`. `body_end` is the index of the next
/// section header (exclusive), or `lines.len()` for the last section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionRange {
    pub header: usize,
    pub body_start: usize,
    pub body_end: usize,
}

impl Sif {
    /// Parse a SIF file. Accepts either CRLF or LF line endings; records
    /// which was found so `render()` can re-emit consistently.
    pub fn parse(content: &str) -> Self {
        let uses_crlf = content.contains("\r\n");
        // Split on \n; strip a trailing \r if present per line. This
        // handles both CRLF and LF inputs.
        let lines: Vec<String> = content
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();

        // If the file ended with a newline, split() produces a trailing
        // empty string. Keep it — we re-emit faithfully.
        let sections = index_sections(&lines);

        Self {
            lines,
            sections,
            uses_crlf,
        }
    }

    /// Render back to a string with the original line endings.
    pub fn render(&self) -> String {
        let ending = if self.uses_crlf { "\r\n" } else { "\n" };
        let mut out = String::with_capacity(self.lines.iter().map(|l| l.len() + 2).sum());
        let mut iter = self.lines.iter().peekable();
        while let Some(line) = iter.next() {
            out.push_str(line);
            // Don't append a trailing line ending to the very last "phantom"
            // empty line if it represents the file's final newline.
            if iter.peek().is_some() {
                out.push_str(ending);
            }
        }
        out
    }

    /// Find a line within a section whose left-of-`=` token matches `key`
    /// (case-insensitive, whitespace-trimmed). Returns the absolute line
    /// index.
    pub fn find_key_in_section(&self, section: &str, key: &str) -> Option<usize> {
        let range = self.sections.get(section)?;
        for i in range.body_start..range.body_end {
            if line_key_matches(&self.lines[i], key) {
                return Some(i);
            }
        }
        None
    }

    /// Move the lines whose key matches any of `keys` from `from_section`
    /// to `to_section`. Each matched line is appended to the destination
    /// section in the order `keys` lists them, then removed from the
    /// source. Section ranges are recomputed at the end.
    ///
    /// Errors if a key is found in the source but the destination section
    /// doesn't exist, or if a key is duplicated within the source section
    /// (an ambiguous match).
    pub fn move_keys(
        &mut self,
        from_section: &str,
        to_section: &str,
        keys: &[&str],
    ) -> Result<usize, String> {
        if !self.sections.contains_key(to_section) {
            return Err(format!("destination section [{to_section}] not found"));
        }
        // Collect the line content for each matched key, in order.
        let mut moved: Vec<String> = Vec::with_capacity(keys.len());
        let mut indices_to_remove: Vec<usize> = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(range) = self.sections.get(from_section) else {
                return Err(format!("source section [{from_section}] not found"));
            };
            let mut found: Option<usize> = None;
            for i in range.body_start..range.body_end {
                if line_key_matches(&self.lines[i], key) {
                    if found.is_some() {
                        return Err(format!(
                            "key `{key}` appears multiple times in [{from_section}]; refusing to move (ambiguous)"
                        ));
                    }
                    found = Some(i);
                }
            }
            if let Some(idx) = found {
                moved.push(self.lines[idx].clone());
                indices_to_remove.push(idx);
            }
            // If not found, skip silently — the key may not be present in
            // all flavours of XP and that's OK.
        }

        // Append moved lines to the destination section's body. Inserting
        // at body_end shifts source indices when source comes AFTER
        // destination in the file; track this carefully.
        let to_range = self.sections[to_section];
        let insert_at = to_range.body_end;

        // Insert moved lines in order at `insert_at`. After this, line
        // indices >= insert_at have shifted by `moved.len()`.
        let n_moved = moved.len();
        for (offset, line) in moved.iter().enumerate() {
            self.lines.insert(insert_at + offset, line.clone());
        }

        // Adjust the recorded indices_to_remove for any that came after
        // insert_at (they got shifted by n_moved).
        let mut adjusted: Vec<usize> = indices_to_remove
            .into_iter()
            .map(|i| if i >= insert_at { i + n_moved } else { i })
            .collect();
        // Remove from highest to lowest so earlier indices stay valid.
        adjusted.sort_unstable_by(|a, b| b.cmp(a));
        for idx in adjusted {
            self.lines.remove(idx);
        }

        // Recompute sections after structural changes.
        self.sections = index_sections(&self.lines);
        Ok(n_moved)
    }

    /// Ensure `key = value` exists in `section`. If the section doesn't
    /// have a matching key, appends `key = value` at the end of the
    /// section's body. If a matching key exists, leaves the existing line
    /// alone (we never silently change values).
    pub fn ensure_kvp(&mut self, section: &str, key: &str, value: &str) -> Result<(), String> {
        if self.find_key_in_section(section, key).is_some() {
            return Ok(());
        }
        let Some(range) = self.sections.get(section).copied() else {
            return Err(format!("section [{section}] not found"));
        };
        let new_line = format!("{key} = {value}");
        self.lines.insert(range.body_end, new_line);
        self.sections = index_sections(&self.lines);
        Ok(())
    }
}

/// Declare a WaitBT/Wait4UFD-style waiter driver in the SIF: copy the
/// .sys filename into [SourceDisksFiles] (so setup knows it exists),
/// add it to [BootBusExtenders.Load] (so it loads at boot), and add a
/// descriptive entry to [BootBusExtenders].
///
/// `key` is the driver name without ".sys" (e.g. "WaitBT", "Wait4UFD").
/// `description` is the human-readable label for [BootBusExtenders].
pub fn declare_waiter(sif: &mut Sif, key: &str, description: &str) -> Result<(), String> {
    let sys_filename = format!("{key}.sys");
    // [SourceDisksFiles]: tell setup the file exists. The trailing column
    // pattern matches what newer SIF entries use: `1,,,,,,_x,4,1,3,,1,4`
    // is the "BootBusExtenders flavor" tag that XP setup recognizes.
    sif.ensure_kvp(
        "SourceDisksFiles",
        &sys_filename,
        "1,,,,,,_x,4,1,3,,1,4",
    )?;
    // [BootBusExtenders.Load]: load at boot.
    sif.ensure_kvp("BootBusExtenders.Load", key, &sys_filename)?;
    // [BootBusExtenders]: descriptive entry. The third column "files.none"
    // is a stub file-group; the waiter isn't part of any larger driver
    // package so there's no specific files.<x> section to reference.
    sif.ensure_kvp(
        "BootBusExtenders",
        key,
        &format!(r#""{description}",files.none,{key}"#),
    )?;
    Ok(())
}

/// Declare `ren_fold.cmd` and `undoren.cmd` in `[SourceDisksFiles]` so
/// text-mode setup recognises them as install-media files and copies them
/// out of `\$WIN_NT$.~LS\I386\` to the spot that `[SetupParams] UserExecute`
/// invokes them from.
///
/// The flag string `100,,,,,,_x,2,0,0` matches the canonical WinSetupFromUSB
/// declaration:
///   - `100` = source disk 1, subdirectory `\I386\`
///   - `_x`  = upload flags reserved by setup (verbatim from USB_MultiBoot)
///   - `2,0,0` = copy to text-mode-target (TempDir), no special handling
///
/// Idempotent — calling twice leaves the file with one entry each.
pub fn declare_ren_scripts(sif: &mut Sif) -> Result<(), String> {
    sif.ensure_kvp("SourceDisksFiles", "ren_fold.cmd", "100,,,,,,_x,2,0,0")?;
    sif.ensure_kvp("SourceDisksFiles", "undoren.cmd", "100,,,,,,_x,2,0,0")?;
    Ok(())
}

/// The 5 USB driver keys the WinSetupFromUSB recipe moves from
/// `InputDevicesSupport.Load` into `BootBusExtenders.Load`. Exposed so the
/// post-write verifier can check the same set the modifier touched.
pub const USB_BOOT_DRIVERS: &[&str] =
    &["usbehci", "usbohci", "usbuhci", "usbhub", "usbstor"];

/// Apply the WinSetupFromUSB-style modifications: move 5 USB drivers from
/// `InputDevicesSupport.Load` to `BootBusExtenders.Load`, and ensure each
/// has a descriptive entry in `BootBusExtenders` (the non-`.Load` section).
///
/// Returns the count of driver lines actually moved (may be less than 5
/// if some weren't present in this flavour of XP).
pub fn apply_usb_boot_mods(sif: &mut Sif) -> Result<usize, String> {
    let moved = sif.move_keys(
        "InputDevicesSupport.Load",
        "BootBusExtenders.Load",
        USB_BOOT_DRIVERS,
    )?;

    // Add descriptive entries to [BootBusExtenders] for each driver that
    // doesn't already have one. The values match the descriptive lines
    // used by Microsoft + WinSetupFromUSB.
    let descriptions: &[(&str, &str)] = &[
        ("usbehci", r#""USB 2.0 Enhanced Host Controller",files.usbehci,usbehci"#),
        ("usbohci", r#""USB Open Host Controller",files.usbohci,usbohci"#),
        ("usbuhci", r#""USB Universal Host Controller",files.usbuhci,usbuhci"#),
        ("usbhub",  r#""USB Standard Hub",files.usbhub,usbhub"#),
        ("usbstor", r#""USB Storage Class Driver",files.usbstor,usbstor"#),
    ];
    for (key, val) in descriptions {
        sif.ensure_kvp("BootBusExtenders", key, val)?;
    }
    Ok(moved)
}

/// Re-parse an on-disk SIF and verify the WinSetupFromUSB mods landed.
///
/// `expected_moved` is the count `apply_usb_boot_mods` returned for this
/// file; the verifier checks that exactly those drivers are present in
/// `[BootBusExtenders.Load]` and absent from `[InputDevicesSupport.Load]`.
/// `expected_moved == 0` is rejected by the pipeline before this is called,
/// so any value here represents a real expectation to verify.
///
/// Sources of failure this catches that the in-memory test can't:
///   - `fs::write` returned Ok but the bytes didn't hit the platter
///     (FAT32 cache, mount-point shenanigans, wrong path resolution).
///   - A later pipeline step silently overwrote the file with the
///     pre-modification contents (e.g. a stale ditto source).
///   - A future refactor breaks the section-name contract without
///     anyone noticing because the unit-test fixture still parses.
pub fn verify_usb_boot_mods_persisted(sif: &Sif, expected_moved: usize) -> Result<(), String> {
    let mut in_dest: Vec<&str> = Vec::new();
    let mut still_in_source: Vec<&str> = Vec::new();
    for drv in USB_BOOT_DRIVERS {
        if sif.find_key_in_section("BootBusExtenders.Load", drv).is_some() {
            in_dest.push(drv);
        }
        if sif.find_key_in_section("InputDevicesSupport.Load", drv).is_some() {
            still_in_source.push(drv);
        }
    }
    if in_dest.len() != expected_moved {
        return Err(format!(
            "expected {expected_moved} USB drivers in [BootBusExtenders.Load] on disk; \
             found {} ({:?})",
            in_dest.len(),
            in_dest
        ));
    }
    if !still_in_source.is_empty() {
        return Err(format!(
            "USB drivers still present in [InputDevicesSupport.Load] on disk: {:?}",
            still_in_source
        ));
    }
    Ok(())
}

// ----- internal helpers ----------------------------------------------------

fn index_sections(lines: &[String]) -> BTreeMap<String, SectionRange> {
    let mut sections: BTreeMap<String, SectionRange> = BTreeMap::new();
    let mut current: Option<(String, usize)> = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(name) = section_header(line) {
            if let Some((prev_name, prev_header)) = current.take() {
                sections.insert(
                    prev_name,
                    SectionRange {
                        header: prev_header,
                        body_start: prev_header + 1,
                        body_end: i,
                    },
                );
            }
            current = Some((name.to_string(), i));
        }
    }
    if let Some((name, header)) = current {
        sections.insert(
            name,
            SectionRange {
                header,
                body_start: header + 1,
                body_end: lines.len(),
            },
        );
    }
    sections
}

/// Return the section name if this line is a `[Name]` header, else None.
/// Tolerates leading/trailing whitespace inside the brackets.
fn section_header(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let inside = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    Some(inside.trim())
}

/// Does the line's `key = value` left-hand-side match `key` (case-
/// insensitive, whitespace-trimmed)?
fn line_key_matches(line: &str, key: &str) -> bool {
    // Strip comments (everything after the first `;`).
    let no_comment = line.split(';').next().unwrap_or(line);
    let Some(equals) = no_comment.find('=') else {
        return false;
    };
    let lhs = no_comment[..equals].trim();
    lhs.eq_ignore_ascii_case(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
[Version]\r\n\
signature=\"$Windows NT$\"\r\n\
\r\n\
[BootBusExtenders.Load]\r\n\
pci      = pci.sys\r\n\
acpi     = acpi.sys\r\n\
\r\n\
[InputDevicesSupport.Load]\r\n\
usbehci  = usbehci.sys\r\n\
usbohci  = usbohci.sys\r\n\
usbuhci  = usbuhci.sys\r\n\
usbhub   = usbhub.sys\r\n\
hidusb   = hidusb.sys\r\n\
usbstor  = usbstor.sys\r\n\
\r\n\
[BootBusExtenders]\r\n\
pci      = \"PCI Bus Driver\",files.pci,pci\r\n\
acpi     = \"ACPI Plug & Play Bus Driver\",files.acpi,acpi\r\n";

    #[test]
    fn parse_and_render_round_trip() {
        let sif = Sif::parse(SAMPLE);
        assert_eq!(sif.render(), SAMPLE);
    }

    #[test]
    fn sections_indexed_correctly() {
        let sif = Sif::parse(SAMPLE);
        assert!(sif.sections.contains_key("Version"));
        assert!(sif.sections.contains_key("BootBusExtenders.Load"));
        assert!(sif.sections.contains_key("InputDevicesSupport.Load"));
        assert!(sif.sections.contains_key("BootBusExtenders"));
    }

    #[test]
    fn find_key_in_section() {
        let sif = Sif::parse(SAMPLE);
        let idx = sif
            .find_key_in_section("InputDevicesSupport.Load", "usbehci")
            .expect("found");
        assert!(sif.lines[idx].contains("usbehci.sys"));
    }

    #[test]
    fn move_keys_basic() {
        let mut sif = Sif::parse(SAMPLE);
        let moved = sif
            .move_keys(
                "InputDevicesSupport.Load",
                "BootBusExtenders.Load",
                &["usbehci", "usbohci", "usbuhci", "usbhub", "usbstor"],
            )
            .unwrap();
        assert_eq!(moved, 5);
        // usbehci no longer in InputDevicesSupport.Load
        assert!(sif
            .find_key_in_section("InputDevicesSupport.Load", "usbehci")
            .is_none());
        // ...and now in BootBusExtenders.Load
        assert!(sif
            .find_key_in_section("BootBusExtenders.Load", "usbehci")
            .is_some());
        // hidusb is still in InputDevicesSupport.Load (we didn't move it)
        assert!(sif
            .find_key_in_section("InputDevicesSupport.Load", "hidusb")
            .is_some());
    }

    #[test]
    fn ensure_kvp_adds_when_missing() {
        let mut sif = Sif::parse(SAMPLE);
        sif.ensure_kvp(
            "BootBusExtenders",
            "usbstor",
            r#""USB Storage Class Driver",files.usbstor,usbstor"#,
        )
        .unwrap();
        let idx = sif
            .find_key_in_section("BootBusExtenders", "usbstor")
            .expect("found");
        assert!(sif.lines[idx].contains("USB Storage"));
    }

    #[test]
    fn ensure_kvp_is_idempotent() {
        let mut sif = Sif::parse(SAMPLE);
        // pci is already in [BootBusExtenders]; ensure_kvp shouldn't change it.
        let original_idx = sif
            .find_key_in_section("BootBusExtenders", "pci")
            .unwrap();
        let original_line = sif.lines[original_idx].clone();
        sif.ensure_kvp("BootBusExtenders", "pci", r#""totally different value""#)
            .unwrap();
        // Should still have the original line, not changed.
        let still = sif
            .find_key_in_section("BootBusExtenders", "pci")
            .unwrap();
        assert_eq!(sif.lines[still], original_line);
    }

    #[test]
    fn declare_ren_scripts_adds_both() {
        let mut sif = Sif::parse(
            "[Version]\r\n\
             signature=\"$Windows NT$\"\r\n\
             \r\n\
             [SourceDisksFiles]\r\n\
             ntoskrnl.exe = 1,,,,,,3_,2,0,0\r\n",
        );
        declare_ren_scripts(&mut sif).unwrap();
        let idx_ren = sif
            .find_key_in_section("SourceDisksFiles", "ren_fold.cmd")
            .expect("ren_fold.cmd added");
        let idx_undo = sif
            .find_key_in_section("SourceDisksFiles", "undoren.cmd")
            .expect("undoren.cmd added");
        assert!(sif.lines[idx_ren].contains("100,,,,,,_x,2,0,0"));
        assert!(sif.lines[idx_undo].contains("100,,,,,,_x,2,0,0"));
        // Idempotent.
        declare_ren_scripts(&mut sif).unwrap();
        let count_ren = sif
            .lines
            .iter()
            .filter(|l| line_key_matches(l, "ren_fold.cmd"))
            .count();
        assert_eq!(count_ren, 1, "second call duplicated ren_fold.cmd");
    }

    #[test]
    fn apply_usb_boot_mods_full() {
        let mut sif = Sif::parse(SAMPLE);
        let n = apply_usb_boot_mods(&mut sif).unwrap();
        assert_eq!(n, 5);
        for driver in &["usbehci", "usbohci", "usbuhci", "usbhub", "usbstor"] {
            assert!(
                sif.find_key_in_section("BootBusExtenders.Load", driver).is_some(),
                "{driver} missing from BootBusExtenders.Load"
            );
            assert!(
                sif.find_key_in_section("InputDevicesSupport.Load", driver).is_none(),
                "{driver} still in InputDevicesSupport.Load"
            );
            assert!(
                sif.find_key_in_section("BootBusExtenders", driver).is_some(),
                "{driver} description missing from BootBusExtenders"
            );
        }
    }

    #[test]
    fn verify_persisted_accepts_well_formed_output() {
        let mut sif = Sif::parse(SAMPLE);
        let moved = apply_usb_boot_mods(&mut sif).unwrap();
        // Round-trip through the renderer like the real pipeline does.
        let reparsed = Sif::parse(&sif.render());
        verify_usb_boot_mods_persisted(&reparsed, moved).expect("clean SIF should verify");
    }

    #[test]
    fn verify_persisted_detects_unmoved_drivers() {
        // SAMPLE has 5 drivers in InputDevicesSupport.Load and none in
        // BootBusExtenders.Load. If apply_usb_boot_mods silently no-op'd
        // (e.g. wrong section name in a future refactor), the verifier
        // must catch that — claim moved=5 but the file is unmodified.
        let sif = Sif::parse(SAMPLE);
        let err = verify_usb_boot_mods_persisted(&sif, 5).unwrap_err();
        assert!(err.contains("BootBusExtenders.Load"), "got: {err}");
    }

    #[test]
    fn verify_persisted_detects_partial_persistence() {
        // Simulate a write that landed for some drivers but not others
        // (e.g. truncated write, FAT32 cache flush partway through). Move
        // 5 in-memory, then strip one out and re-parse.
        let mut sif = Sif::parse(SAMPLE);
        let moved = apply_usb_boot_mods(&mut sif).unwrap();
        assert_eq!(moved, 5);
        let mut rendered = sif.render();
        // Remove the usbstor line from the rendered output to mimic a
        // partial persistence failure.
        let usbstor_line = sif
            .lines
            .iter()
            .find(|l| line_key_matches(l, "usbstor"))
            .unwrap()
            .clone();
        rendered = rendered.replace(&format!("{usbstor_line}\r\n"), "");
        let reparsed = Sif::parse(&rendered);
        let err = verify_usb_boot_mods_persisted(&reparsed, 5).unwrap_err();
        assert!(err.contains("found 4"), "got: {err}");
    }

    /// Apply the mods to the real XP SP3 fixture and re-parse, verifying
    /// structural integrity. Slow-ish (22k lines); kept in default test set
    /// because it catches real-world parser bugs that the synthetic SAMPLE
    /// can't.
    #[test]
    fn real_xp_sp3_fixture_round_trips() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/xp_sp3/txtsetup.sif");
        let original = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("skipping: fixture not present at {path}");
                return;
            }
        };
        let mut sif = Sif::parse(&original);
        // Round-trip without mods should be byte-identical.
        assert_eq!(sif.render(), original, "round-trip mismatch on real SIF");

        // Apply mods.
        let moved = apply_usb_boot_mods(&mut sif).expect("apply mods");
        assert_eq!(moved, 5, "expected to move 5 USB drivers");

        // After mods, the file should still parse cleanly.
        let modified = sif.render();
        let reparsed = Sif::parse(&modified);
        assert!(reparsed.sections.contains_key("BootBusExtenders.Load"));
        for driver in &["usbehci", "usbohci", "usbuhci", "usbhub", "usbstor"] {
            assert!(
                reparsed
                    .find_key_in_section("BootBusExtenders.Load", driver)
                    .is_some(),
                "after-mod re-parse: {driver} missing from BootBusExtenders.Load"
            );
        }
        // And the post-write verifier should accept the round-tripped file.
        verify_usb_boot_mods_persisted(&reparsed, moved)
            .expect("real XP SP3 fixture should verify after round-trip");
    }
}
