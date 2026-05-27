//! ISO9660 inspection. Two responsibilities:
//!
//! 1. **Classify** an ISO into one of the four `BootMode` families so the
//!    pipeline knows what to build (auto-mode resolution).
//! 2. **Inspect** an ISO well enough to dry-run the file copy step without
//!    actually mounting it via `hdiutil`.
//!
//! The classifier walks the ISO9660 directory tree and applies the current
//! mode markers:
//!
//! - Hybrid: protective MBR / GPT signature at offset 0x1FE + EFI System
//!   Partition GUID present in protective MBR area.
//! - Windows NT5: contains `I386/TXTSETUP.SIF` plus NT5 loader/marker files.
//! - Windows: contains `bootmgr` AND `sources/install.wim` or
//!   `sources/install.esd`.
//! - IsolinuxLinux: contains `isolinux/isolinux.bin`.
//! - UefiOnly: contains `EFI/BOOT/BOOTX64.EFI` (or other UEFI loader path)
//!   AND lacks an MBR signature.
//!
//! Windows 2000 media (NT 5.0) is split out from XP/2003 (NT 5.1/5.2)
//! using the WIN51/WIN52 root-marker files that Microsoft has shipped on
//! XP-and-later install media since XP launched. Presence = XP/2003 →
//! `WindowsNtXp`; absence (but still NT5-class) = Win2k →
//! `Windows2000`. The two modes share the GRUB4DOS chain shape but
//! differ in the textmode ramdisk driver (FiraDisk vs SVBus).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use thiserror::Error;
use bootsmith_core::plan::BootMode;

const SECTOR_SIZE: u64 = 2048;
const PVD_SECTOR: u64 = 16;
const ISO_ID: &[u8; 5] = b"CD001";
const MAX_DIR_DEPTH: usize = 8;

#[derive(Debug, Error)]
pub enum IsoError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("not a valid ISO9660 image: {0}")]
    NotIso9660(String),

    #[error("cannot determine boot mode automatically; pass --type explicitly")]
    Ambiguous,
}

pub type Result<T> = std::result::Result<T, IsoError>;

/// Inspect an ISO and return the boot mode that should be used.
pub fn classify(path: &Path) -> Result<BootMode> {
    let mut iso = IsoReader::open(path)?;
    let entries = iso.collect_paths()?;

    if is_nt5_install_media(&entries) {
        return Ok(if has_win51_or_win52_marker(&entries) {
            BootMode::WindowsNtXp
        } else {
            BootMode::Windows2000
        });
    }
    if has(&entries, "BOOTMGR")
        && (has(&entries, "SOURCES/INSTALL.WIM") || has(&entries, "SOURCES/INSTALL.ESD"))
    {
        return Ok(BootMode::Windows);
    }
    if has(&entries, "ISOLINUX/ISOLINUX.BIN") {
        return Ok(BootMode::IsolinuxLinux);
    }
    if has(&entries, "EFI/BOOT/BOOTX64.EFI") {
        return Ok(BootMode::UefiOnly);
    }

    Err(IsoError::Ambiguous)
}

fn is_nt5_install_media(entries: &BTreeSet<String>) -> bool {
    has(entries, "I386/TXTSETUP.SIF")
        && (has(entries, "I386/SETUPLDR.BIN")
            || has(entries, "I386/NTDETECT.COM")
            || has_win51_or_win52_marker(entries))
        && !has(entries, "BOOTMGR")
        && !entries.iter().any(|p| p.starts_with("SOURCES/"))
}

fn has_win51_or_win52_marker(entries: &BTreeSet<String>) -> bool {
    entries.iter().any(|p| {
        p == "WIN51" || p.starts_with("WIN51") || p == "WIN52" || p.starts_with("WIN52")
    })
}

fn has(entries: &BTreeSet<String>, path: &str) -> bool {
    entries.contains(path)
}

#[derive(Debug, Clone, Copy)]
struct DirRef {
    extent: u32,
    len: u32,
}

struct IsoReader {
    file: File,
    root: DirRef,
}

impl IsoReader {
    fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let mut pvd = [0u8; SECTOR_SIZE as usize];
        file.seek(SeekFrom::Start(PVD_SECTOR * SECTOR_SIZE))?;
        file.read_exact(&mut pvd)?;

        if pvd[0] != 1 || &pvd[1..6] != ISO_ID {
            return Err(IsoError::NotIso9660(
                "missing primary volume descriptor".into(),
            ));
        }

        let root = parse_dir_record(&pvd[156..])
            .ok_or_else(|| IsoError::NotIso9660("missing root directory record".into()))?
            .dir_ref;

        Ok(Self { file, root })
    }

    fn collect_paths(&mut self) -> Result<BTreeSet<String>> {
        let mut out = BTreeSet::new();
        self.walk_dir(self.root, "", 0, &mut out)?;
        Ok(out)
    }

    fn walk_dir(
        &mut self,
        dir: DirRef,
        prefix: &str,
        depth: usize,
        out: &mut BTreeSet<String>,
    ) -> Result<()> {
        if depth > MAX_DIR_DEPTH {
            return Ok(());
        }

        let bytes = self.read_extent(dir)?;
        let mut offset = 0usize;
        while offset < bytes.len() {
            let len = bytes[offset] as usize;
            if len == 0 {
                offset = ((offset / SECTOR_SIZE as usize) + 1) * SECTOR_SIZE as usize;
                continue;
            }
            if offset + len > bytes.len() {
                break;
            }

            if let Some(record) = parse_dir_record(&bytes[offset..offset + len]) {
                if record.name != "\0" && record.name != "\u{1}" {
                    let path = if prefix.is_empty() {
                        record.name.clone()
                    } else {
                        format!("{prefix}/{}", record.name)
                    };
                    out.insert(path.clone());
                    if record.is_dir {
                        self.walk_dir(record.dir_ref, &path, depth + 1, out)?;
                    }
                }
            }
            offset += len;
        }
        Ok(())
    }

    fn read_extent(&mut self, dir: DirRef) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; dir.len as usize];
        self.file
            .seek(SeekFrom::Start(dir.extent as u64 * SECTOR_SIZE))?;
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }
}

struct DirRecord {
    dir_ref: DirRef,
    is_dir: bool,
    name: String,
}

fn parse_dir_record(bytes: &[u8]) -> Option<DirRecord> {
    if bytes.len() < 34 || bytes[0] as usize > bytes.len() {
        return None;
    }
    let name_len = bytes[32] as usize;
    if 33 + name_len > bytes.len() {
        return None;
    }
    let extent = u32::from_le_bytes(bytes[2..6].try_into().ok()?);
    let len = u32::from_le_bytes(bytes[10..14].try_into().ok()?);
    let is_dir = bytes[25] & 0x02 != 0;
    let raw_name = &bytes[33..33 + name_len];
    let name = normalize_iso_name(raw_name);
    Some(DirRecord {
        dir_ref: DirRef { extent, len },
        is_dir,
        name,
    })
}

fn normalize_iso_name(raw: &[u8]) -> String {
    if raw == [0] {
        return "\0".into();
    }
    if raw == [1] {
        return "\u{1}".into();
    }
    let mut s = String::from_utf8_lossy(raw).to_uppercase();
    if let Some((base, _version)) = s.split_once(';') {
        s = base.to_string();
    }
    s.trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn classifies_nt5_xp_media() {
        let iso = synthetic_iso(&[
            ("I386", true),
            ("I386/TXTSETUP.SIF", false),
            ("I386/SETUPLDR.BIN", false),
            ("I386/NTDETECT.COM", false),
            ("WIN51IP", false),
        ]);
        let path = write_temp_iso("xp", &iso);
        assert_eq!(classify(&path).unwrap(), BootMode::WindowsNtXp);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn classifies_nt5_windows_2000_media_without_win51_marker() {
        let iso = synthetic_iso(&[
            ("I386", true),
            ("I386/TXTSETUP.SIF", false),
            ("I386/SETUPLDR.BIN", false),
            ("I386/NTDETECT.COM", false),
        ]);
        let path = write_temp_iso("win2000", &iso);
        assert_eq!(classify(&path).unwrap(), BootMode::Windows2000);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn classifies_nt5_windows_2003_media_with_win52_marker_as_ntxp() {
        let iso = synthetic_iso(&[
            ("I386", true),
            ("I386/TXTSETUP.SIF", false),
            ("I386/SETUPLDR.BIN", false),
            ("I386/NTDETECT.COM", false),
            ("WIN52", false),
        ]);
        let path = write_temp_iso("win2003", &iso);
        assert_eq!(classify(&path).unwrap(), BootMode::WindowsNtXp);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn classifies_windows_nt6_media() {
        let iso = synthetic_iso(&[
            ("BOOTMGR", false),
            ("SOURCES", true),
            ("SOURCES/INSTALL.WIM", false),
        ]);
        let path = write_temp_iso("win", &iso);
        assert_eq!(classify(&path).unwrap(), BootMode::Windows);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ambiguous_when_markers_missing() {
        let iso = synthetic_iso(&[("README.TXT", false)]);
        let path = write_temp_iso("ambiguous", &iso);
        assert!(matches!(classify(&path), Err(IsoError::Ambiguous)));
        let _ = fs::remove_file(path);
    }

    fn write_temp_iso(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bootsmith_iso_{name}_{}_{}.iso",
            std::process::id(),
            bytes.len()
        ));
        let mut file = File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    fn synthetic_iso(entries: &[(&str, bool)]) -> Vec<u8> {
        let mut iso = vec![0u8; 24 * SECTOR_SIZE as usize];
        let root_sector = 20u32;
        let mut next_sector = 21u32;

        let mut dirs = BTreeSet::new();
        dirs.insert(String::new());
        for (path, is_dir) in entries {
            let parts: Vec<_> = path.split('/').collect();
            if parts.len() > 1 {
                dirs.insert(parts[..parts.len() - 1].join("/"));
            }
            if *is_dir {
                dirs.insert((*path).to_string());
            }
        }

        let mut dir_refs = std::collections::BTreeMap::new();
        dir_refs.insert(String::new(), DirRef { extent: root_sector, len: SECTOR_SIZE as u32 });
        for dir in dirs.iter().filter(|d| !d.is_empty()) {
            dir_refs.insert(dir.clone(), DirRef { extent: next_sector, len: SECTOR_SIZE as u32 });
            next_sector += 1;
        }

        let needed = (next_sector as usize + 1) * SECTOR_SIZE as usize;
        if iso.len() < needed {
            iso.resize(needed, 0);
        }

        write_pvd(&mut iso, dir_refs[""]);

        for dir in &dirs {
            let dir_ref = dir_refs[dir];
            let mut records = Vec::new();
            records.extend(dir_record("\0", dir_ref, true));
            records.extend(dir_record("\u{1}", dir_ref, true));

            for (path, is_dir) in entries {
                let parent = parent_dir(path);
                if parent == *dir {
                    let name = path.rsplit('/').next().unwrap();
                    let child_ref = if *is_dir {
                        dir_refs[*path]
                    } else {
                        DirRef { extent: next_sector, len: 0 }
                    };
                    records.extend(dir_record(name, child_ref, *is_dir));
                }
            }

            let start = dir_ref.extent as usize * SECTOR_SIZE as usize;
            iso[start..start + records.len()].copy_from_slice(&records);
        }

        iso
    }

    fn parent_dir(path: &str) -> String {
        path.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default()
    }

    fn write_pvd(iso: &mut [u8], root: DirRef) {
        let start = PVD_SECTOR as usize * SECTOR_SIZE as usize;
        iso[start] = 1;
        iso[start + 1..start + 6].copy_from_slice(ISO_ID);
        iso[start + 6] = 1;
        let rec = dir_record("\0", root, true);
        iso[start + 156..start + 156 + rec.len()].copy_from_slice(&rec);
    }

    fn dir_record(name: &str, dir_ref: DirRef, is_dir: bool) -> Vec<u8> {
        let name_bytes: Vec<u8> = match name {
            "\0" => vec![0],
            "\u{1}" => vec![1],
            other => other.as_bytes().to_vec(),
        };
        let mut len = 33 + name_bytes.len();
        if len % 2 != 0 {
            len += 1;
        }
        let mut rec = vec![0u8; len];
        rec[0] = len as u8;
        rec[2..6].copy_from_slice(&dir_ref.extent.to_le_bytes());
        rec[10..14].copy_from_slice(&dir_ref.len.to_le_bytes());
        rec[25] = if is_dir { 0x02 } else { 0x00 };
        rec[28..30].copy_from_slice(&1u16.to_le_bytes());
        rec[32] = name_bytes.len() as u8;
        rec[33..33 + name_bytes.len()].copy_from_slice(&name_bytes);
        rec
    }
}
