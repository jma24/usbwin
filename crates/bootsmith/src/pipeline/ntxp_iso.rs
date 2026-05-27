//! Minimal ISO9660 mutation for NT5 unattended setup.
//!
//! XP setup consumes `I386\WINNT.SIF` from the booted setup source. The
//! GRUB4DOS/FiraDisk pipeline must therefore place the generated answer file
//! inside the staged `XP.ISO` without moving the XP El Torito boot image. This
//! module implements the narrow append-only case verified on the XP SP3 VL ISO:
//! grow the existing `I386` directory size into zero padding at the end of its
//! final sector, add one `WINNT.SIF;1` record there, and append the file data at
//! the end of the ISO.

use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const SECTOR_SIZE: u64 = 2048;
const PVD_LBA: u64 = 16;
const PVD_OFFSET: u64 = PVD_LBA * SECTOR_SIZE;
const ROOT_RECORD_OFFSET_IN_PVD: usize = 156;
const VOLUME_SPACE_SIZE_OFFSET_IN_PVD: u64 = PVD_OFFSET + 80;
const DIRECTORY_FLAG: u8 = 0x02;
const WINNT_SIF_NAME: &[u8] = b"WINNT.SIF;1";

/// Append a new file into the staged ISO's `I386` directory.
///
/// The file's data goes at the end of the ISO (aligned to a fresh
/// sector); a new directory record is written into the I386 directory's
/// existing allocated extent, growing its `data_len` to cover the new
/// record but **not** reallocating the directory to a different extent.
/// That means the I386 directory's allocated sector range must already
/// have enough slack at the end -- typical XP SP3 ISOs do, but a packed
/// directory would bail with a clear error.
///
/// `file_name` is the ISO9660 long-form name including the `;1` version
/// suffix (e.g. `b"IASTOR.SYS;1"`). Returns an error if a record with
/// that name already exists in the directory.
pub fn append_file_to_i386(iso_path: &Path, file_name: &[u8], contents: &[u8]) -> Result<()> {
    let mut file = open_iso_rw(iso_path)?;
    append_file_into_i386(&mut file, file_name, contents)?;
    file.sync_all()?;
    Ok(())
}

/// Append a new file into the staged ISO's root directory.
///
/// Mirrors `append_file_to_i386` but targets the volume root instead of
/// `\I386`. Built for the AHCI slipstream path to drop a copy of
/// `iaStor.sys` alongside `\I386\IASTOR.SYS` so XP GUI-mode setup's
/// "Files Needed" dialog (which defaults its source path to the install
/// media root `F:\`) would find the file without prompting the user to
/// browse manually to `F:\i386`.
///
/// Currently unused in the pipeline: hardware testing on 2026-05-26
/// found that any modification to the ISO root directory layout (this
/// helper, or the earlier OemPnPDriversPath sif injection) breaks XP
/// GUI-mode setup's side-by-side assembly copy phase, surfacing as the
/// "The file 'asms' on Windows XP Professional Service Pack 3 CD is
/// needed" prompt mid-file-copy. The exact root-directory invariant
/// XP cares about is unknown. Kept as a library primitive (tested)
/// for future work that figures the regression out.
#[allow(dead_code)]
pub fn append_file_to_root(iso_path: &Path, file_name: &[u8], contents: &[u8]) -> Result<()> {
    let mut file = open_iso_rw(iso_path)?;
    append_file_into_root(&mut file, file_name, contents)?;
    file.sync_all()?;
    Ok(())
}

/// Read the contents of an existing file in the staged ISO's `I386`
/// directory. Returns `Ok(None)` if no file with that name exists.
///
/// `file_name` is the ISO9660 long-form name including the `;1` version
/// suffix (e.g. `b"TXTSETUP.SIF;1"`).
pub fn read_file_from_i386(iso_path: &Path, file_name: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut file = open_iso_rw(iso_path)?;
    let i386 = find_i386_record(&mut file)?;
    let records = read_directory_records(&mut file, &i386)?;
    let Some(record) = records.iter().find(|r| iso_name_eq(&r.name, file_name)) else {
        return Ok(None);
    };
    let start = u64::from(record.extent_lba) * SECTOR_SIZE;
    let mut buf = vec![0u8; record.data_len as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut buf)?;
    Ok(Some(buf))
}

/// Replace the contents of an existing file in the staged ISO's `I386`
/// directory.
///
/// The new content is appended at the end of the ISO (fresh extent);
/// the existing directory record is updated in place to point at the
/// new extent with the new size. The old extent is orphaned -- ISO9660
/// has no free-space map, so unreferenced extents simply persist as
/// dead bytes on disk. That's fine for our pipeline since the staged
/// XP.ISO is single-use.
///
/// `file_name` is the ISO9660 long-form name including `;1`. Returns an
/// error if no record with that name is found.
pub fn replace_file_in_i386(iso_path: &Path, file_name: &[u8], contents: &[u8]) -> Result<()> {
    let mut file = open_iso_rw(iso_path)?;
    replace_file_into_i386(&mut file, file_name, contents)?;
    file.sync_all()?;
    Ok(())
}

fn open_iso_rw(iso_path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(iso_path)
        .with_context(|| format!("open {}", iso_path.display()))?;
    let iso_len = file
        .metadata()
        .with_context(|| format!("stat {}", iso_path.display()))?
        .len();
    if iso_len < (PVD_LBA + 1) * SECTOR_SIZE {
        bail!("{} is too small to be an ISO9660 image", iso_path.display());
    }
    Ok(file)
}

fn find_i386_record(file: &mut File) -> Result<DirectoryRecord> {
    let pvd = read_sector(file, PVD_LBA).context("read ISO9660 primary volume descriptor")?;
    validate_pvd(&pvd)?;
    let root = parse_directory_record(&pvd, ROOT_RECORD_OFFSET_IN_PVD, PVD_OFFSET)
        .context("parse ISO9660 root directory record")?;
    if root.flags & DIRECTORY_FLAG == 0 {
        bail!("ISO9660 root record is not a directory");
    }
    let i386 = find_child_record(file, &root, b"I386")
        .context("locate ISO9660 /I386 directory")?
        .ok_or_else(|| anyhow::anyhow!("ISO9660 /I386 directory not found"))?;
    if i386.flags & DIRECTORY_FLAG == 0 {
        bail!("ISO9660 /I386 is not a directory");
    }
    Ok(i386)
}

fn find_root_record(file: &mut File) -> Result<DirectoryRecord> {
    let pvd = read_sector(file, PVD_LBA).context("read ISO9660 primary volume descriptor")?;
    validate_pvd(&pvd)?;
    let root = parse_directory_record(&pvd, ROOT_RECORD_OFFSET_IN_PVD, PVD_OFFSET)
        .context("parse ISO9660 root directory record")?;
    if root.flags & DIRECTORY_FLAG == 0 {
        bail!("ISO9660 root record is not a directory");
    }
    Ok(root)
}

fn append_file_into_root(file: &mut File, file_name: &[u8], contents: &[u8]) -> Result<()> {
    if contents.is_empty() {
        bail!(
            "refusing to append zero-length file {}",
            String::from_utf8_lossy(file_name)
        );
    }
    let root = find_root_record(file)?;
    let records = read_directory_records(file, &root)?;
    if records
        .iter()
        .any(|record| iso_name_eq(&record.name, file_name))
    {
        bail!(
            "ISO9660 /{} already exists",
            String::from_utf8_lossy(file_name)
        );
    }

    let root_start = u64::from(root.extent_lba) * SECTOR_SIZE;
    let old_data_len = u64::from(root.data_len);
    let allocated = align_up(old_data_len, SECTOR_SIZE);

    let iso_len = file.metadata()?.len();
    let new_extent_lba = u32::try_from(align_up(iso_len, SECTOR_SIZE) / SECTOR_SIZE)
        .context("patched ISO would exceed 32-bit ISO9660 LBA range")?;
    let new_data_len = u32::try_from(contents.len()).with_context(|| {
        format!(
            "file {} too large for ISO9660",
            String::from_utf8_lossy(file_name)
        )
    })?;
    let record = build_file_record(file_name, new_extent_lba, new_data_len)?;

    let new_dir_data_len = rewrite_directory_with_added_record(
        file, root_start, allocated, &records, record,
    )?;

    // Update root's data_len in TWO places:
    // 1. The PVD's root-directory record (offset 156 inside the PVD).
    // 2. The root's own self-record (the "." entry at the start of root's
    //    data extent).
    let new_dir_data_len_u32 = u32::try_from(new_dir_data_len)
        .context("ISO9660 root directory too large for 32-bit data_len")?;
    write_733_at(
        file,
        PVD_OFFSET + ROOT_RECORD_OFFSET_IN_PVD as u64 + 10,
        new_dir_data_len_u32,
    )
    .context("update PVD root directory record size")?;
    write_733_at(file, root_start + 10, new_dir_data_len_u32)
        .context("update root self directory record size")?;

    write_file_contents_at_end(file, new_extent_lba, contents)?;
    Ok(())
}

fn append_file_into_i386(file: &mut File, file_name: &[u8], contents: &[u8]) -> Result<()> {
    if contents.is_empty() {
        bail!(
            "refusing to append zero-length file {}",
            String::from_utf8_lossy(file_name)
        );
    }
    let i386 = find_i386_record(file)?;
    let records = read_directory_records(file, &i386)?;
    if records
        .iter()
        .any(|record| iso_name_eq(&record.name, file_name))
    {
        bail!(
            "ISO9660 /I386/{} already exists",
            String::from_utf8_lossy(file_name)
        );
    }

    let i386_start = u64::from(i386.extent_lba) * SECTOR_SIZE;
    let old_data_len = u64::from(i386.data_len);
    let allocated = align_up(old_data_len, SECTOR_SIZE);

    let iso_len = file.metadata()?.len();
    let new_extent_lba = u32::try_from(align_up(iso_len, SECTOR_SIZE) / SECTOR_SIZE)
        .context("patched ISO would exceed 32-bit ISO9660 LBA range")?;
    let new_data_len = u32::try_from(contents.len()).with_context(|| {
        format!(
            "file {} too large for ISO9660",
            String::from_utf8_lossy(file_name)
        )
    })?;
    let record = build_file_record(file_name, new_extent_lba, new_data_len)?;

    let new_dir_data_len =
        rewrite_directory_with_added_record(file, i386_start, allocated, &records, record)?;

    // Bump the directory's data_len in BOTH the root's record AND I386's
    // self-record so the size stays consistent. The extent_lba doesn't
    // move; only the in-extent slot count changes.
    let new_dir_data_len_u32 = u32::try_from(new_dir_data_len)
        .context("ISO9660 /I386 directory too large for 32-bit data_len")?;
    write_733_at(file, i386.offset + 10, new_dir_data_len_u32)
        .context("update root /I386 directory record size")?;
    write_733_at(file, i386_start + 10, new_dir_data_len_u32)
        .context("update /I386 self directory record size")?;

    // Append file data at end of ISO, aligned to sector.
    write_file_contents_at_end(file, new_extent_lba, contents)?;
    Ok(())
}

fn rewrite_directory_with_added_record(
    file: &mut File,
    dir_start: u64,
    allocated: u64,
    records: &[DirectoryRecord],
    new_record: Vec<u8>,
) -> Result<u64> {
    let mut materialized: Vec<Vec<u8>> = Vec::with_capacity(records.len() + 1);
    for record in records {
        file.seek(SeekFrom::Start(record.offset))?;
        let mut bytes = vec![0u8; record.len as usize];
        file.read_exact(&mut bytes)?;
        materialized.push(bytes);
    }
    materialized.push(new_record);

    if materialized.len() > 2 {
        let (_dot_entries, sortable) = materialized.split_at_mut(2);
        sortable.sort_by(|a, b| {
            let an = iso_sort_key(record_name(a));
            let bn = iso_sort_key(record_name(b));
            an.cmp(&bn)
        });
    }

    let mut dir = vec![0u8; allocated as usize];
    let mut pos = 0usize;
    for record in &materialized {
        let in_sector = pos % SECTOR_SIZE as usize;
        if in_sector + record.len() > SECTOR_SIZE as usize {
            pos += SECTOR_SIZE as usize - in_sector;
        }
        if pos + record.len() > dir.len() {
            bail!(
                "ISO9660 directory at LBA {} has no room for new sorted record set \
                 ({} bytes allocated; next record needs {} bytes at offset {})",
                dir_start / SECTOR_SIZE,
                allocated,
                record.len(),
                pos
            );
        }
        dir[pos..pos + record.len()].copy_from_slice(record);
        pos += record.len();
    }

    file.seek(SeekFrom::Start(dir_start))?;
    file.write_all(&dir)?;
    Ok(pos as u64)
}

fn replace_file_into_i386(file: &mut File, file_name: &[u8], contents: &[u8]) -> Result<()> {
    if contents.is_empty() {
        bail!(
            "refusing to replace {} with zero-length content",
            String::from_utf8_lossy(file_name)
        );
    }
    let i386 = find_i386_record(file)?;
    let records = read_directory_records(file, &i386)?;
    let existing = records
        .iter()
        .find(|record| iso_name_eq(&record.name, file_name))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "ISO9660 /I386/{} not found",
                String::from_utf8_lossy(file_name)
            )
        })?;

    let iso_len = file.metadata()?.len();
    let new_extent_lba = u32::try_from(align_up(iso_len, SECTOR_SIZE) / SECTOR_SIZE)
        .context("patched ISO would exceed 32-bit ISO9660 LBA range")?;
    let new_data_len = u32::try_from(contents.len()).with_context(|| {
        format!(
            "file {} too large for ISO9660",
            String::from_utf8_lossy(file_name)
        )
    })?;

    // Update extent_lba (offset 2..10) and data_len (offset 10..18) of
    // the existing record. Name, dates, etc. stay verbatim.
    write_733_at(file, existing.offset + 2, new_extent_lba)
        .context("update existing record extent_lba")?;
    write_733_at(file, existing.offset + 10, new_data_len)
        .context("update existing record data_len")?;

    write_file_contents_at_end(file, new_extent_lba, contents)?;
    Ok(())
}

/// Append `contents` to the ISO at the sector specified by
/// `new_extent_lba` (must equal `align_up(current_len, SECTOR_SIZE) /
/// SECTOR_SIZE`). Zero-pads to the next sector boundary; updates the
/// PVD volume size accordingly.
fn write_file_contents_at_end(file: &mut File, new_extent_lba: u32, contents: &[u8]) -> Result<()> {
    let iso_len = file.metadata()?.len();
    let file_data_offset = u64::from(new_extent_lba) * SECTOR_SIZE;
    if file_data_offset < iso_len {
        bail!(
            "internal error: new extent LBA {new_extent_lba} maps to {file_data_offset} but ISO is already {iso_len} bytes"
        );
    }

    let final_len = align_up(file_data_offset + contents.len() as u64, SECTOR_SIZE);
    let final_sectors_u32 =
        u32::try_from(final_len / SECTOR_SIZE).context("patched ISO exceeds 32-bit volume size")?;

    file.seek(SeekFrom::Start(iso_len))?;
    if file_data_offset > iso_len {
        write_zeroes(file, file_data_offset - iso_len)?;
    }
    file.write_all(contents)?;
    if final_len > file_data_offset + contents.len() as u64 {
        write_zeroes(file, final_len - file_data_offset - contents.len() as u64)?;
    }
    file.set_len(final_len)?;

    write_733_at(file, VOLUME_SPACE_SIZE_OFFSET_IN_PVD, final_sectors_u32)
        .context("update ISO9660 volume size")?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DirectoryRecord {
    offset: u64,
    len: u8,
    extent_lba: u32,
    data_len: u32,
    flags: u8,
    name: Vec<u8>,
}

pub fn inject_winnt_sif(iso_path: &Path, contents: &[u8]) -> Result<()> {
    if contents.is_empty() {
        bail!("generated WINNT.SIF is empty");
    }
    // Route through the sorted-append primitive so I386 stays
    // alphabetically sorted regardless of what other mutations
    // (e.g. AHCI slipstream) ran before us.
    //
    // The old `inject_winnt_sif` implementation appended at the literal
    // end of the directory without sorting. That was fine when winnt.sif
    // was the only modification; combined with the AHCI slipstream
    // (which sorts the directory), winnt.sif at the literal end leaves
    // the directory unsorted at the W/Z boundary -- which surfaces as
    // the "The file 'asms' on Windows XP CD is needed" prompt during
    // GUI-mode setup. See `reference_xp_iso_root_invariant.md` and
    // `reference_iso9660_dir_sort.md` for the chain of debugging.
    append_file_to_i386(iso_path, WINNT_SIF_NAME, contents)
}

fn validate_pvd(sector: &[u8]) -> Result<()> {
    if sector.len() != SECTOR_SIZE as usize {
        bail!("short ISO9660 primary volume descriptor");
    }
    if sector[0] != 1 || &sector[1..6] != b"CD001" || sector[6] != 1 {
        bail!("ISO9660 primary volume descriptor not found at sector 16");
    }
    Ok(())
}

fn find_child_record(
    file: &mut File,
    directory: &DirectoryRecord,
    name: &[u8],
) -> Result<Option<DirectoryRecord>> {
    for record in read_directory_records(file, directory)? {
        if iso_name_eq(&record.name, name) {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

fn read_directory_records(
    file: &mut File,
    directory: &DirectoryRecord,
) -> Result<Vec<DirectoryRecord>> {
    let start = u64::from(directory.extent_lba) * SECTOR_SIZE;
    let end = start + u64::from(directory.data_len);
    let mut records = Vec::new();
    let mut pos = start;
    while pos < end {
        file.seek(SeekFrom::Start(pos))?;
        let mut len = [0u8; 1];
        file.read_exact(&mut len)?;
        if len[0] == 0 {
            pos = align_up(pos + 1, SECTOR_SIZE);
            continue;
        }
        let len_usize = len[0] as usize;
        let remaining = end - pos;
        if len_usize as u64 > remaining {
            bail!("ISO9660 directory record at byte {pos} exceeds directory extent");
        }
        let mut buf = vec![0u8; len_usize];
        buf[0] = len[0];
        file.read_exact(&mut buf[1..])?;
        records.push(parse_directory_record(&buf, 0, pos)?);
        pos += len_usize as u64;
    }
    Ok(records)
}

fn parse_directory_record(
    buf: &[u8],
    offset: usize,
    absolute_base: u64,
) -> Result<DirectoryRecord> {
    if offset >= buf.len() {
        bail!("directory record offset out of range");
    }
    let len = buf[offset];
    if len < 34 {
        bail!(
            "ISO9660 directory record at byte {} is too short",
            absolute_base + offset as u64
        );
    }
    let end = offset + len as usize;
    if end > buf.len() {
        bail!(
            "ISO9660 directory record at byte {} is truncated",
            absolute_base + offset as u64
        );
    }
    let name_len = buf[offset + 32] as usize;
    if offset + 33 + name_len > end {
        bail!(
            "ISO9660 directory record at byte {} has truncated name",
            absolute_base + offset as u64
        );
    }
    let extent_lba = read_733(&buf[offset + 2..offset + 10])?;
    let data_len = read_733(&buf[offset + 10..offset + 18])?;
    Ok(DirectoryRecord {
        offset: absolute_base + offset as u64,
        len,
        extent_lba,
        data_len,
        flags: buf[offset + 25],
        name: buf[offset + 33..offset + 33 + name_len].to_vec(),
    })
}

fn read_733(bytes: &[u8]) -> Result<u32> {
    if bytes.len() != 8 {
        bail!("invalid ISO9660 733 field length");
    }
    let le = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let be = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
    if le != be {
        bail!("ISO9660 both-endian field mismatch: little={le}, big={be}");
    }
    Ok(le)
}

fn build_file_record(name: &[u8], extent_lba: u32, data_len: u32) -> Result<Vec<u8>> {
    let len = 33 + name.len() + ((33 + name.len()) % 2);
    let len_u8 = u8::try_from(len).context("ISO9660 file name is too long")?;
    let mut record = vec![0u8; len];
    record[0] = len_u8;
    write_733(&mut record[2..10], extent_lba);
    write_733(&mut record[10..18], data_len);
    record[18] = 126; // 2026 - 1900
    record[19] = 5;
    record[20] = 21;
    record[21] = 0;
    record[22] = 0;
    record[23] = 0;
    record[24] = 0;
    record[25] = 0;
    write_723(&mut record[28..32], 1);
    record[32] = u8::try_from(name.len()).context("ISO9660 file name is too long")?;
    record[33..33 + name.len()].copy_from_slice(name);
    Ok(record)
}

fn record_name(record: &[u8]) -> &[u8] {
    let name_len = record.get(32).copied().unwrap_or(0) as usize;
    if record.len() < 33 + name_len {
        b""
    } else {
        &record[33..33 + name_len]
    }
}

fn iso_sort_key(name: &[u8]) -> Vec<u8> {
    name.strip_suffix(b";1")
        .unwrap_or(name)
        .iter()
        .map(|b| b.to_ascii_uppercase())
        .collect()
}

fn read_sector(file: &mut File, lba: u64) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; SECTOR_SIZE as usize];
    file.seek(SeekFrom::Start(lba * SECTOR_SIZE))?;
    file.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_733_at(file: &mut File, offset: u64, value: u32) -> Result<()> {
    let mut buf = [0u8; 8];
    write_733(&mut buf, value);
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&buf)?;
    Ok(())
}

fn write_733(buf: &mut [u8], value: u32) {
    buf[0..4].copy_from_slice(&value.to_le_bytes());
    buf[4..8].copy_from_slice(&value.to_be_bytes());
}

fn write_723(buf: &mut [u8], value: u16) {
    buf[0..2].copy_from_slice(&value.to_le_bytes());
    buf[2..4].copy_from_slice(&value.to_be_bytes());
}

#[allow(dead_code)]
fn ensure_zero_range(file: &mut File, offset: u64, len: u64) -> Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    let mut remaining = len;
    let mut buf = [0u8; 4096];
    while remaining > 0 {
        let n = remaining.min(buf.len() as u64) as usize;
        file.read_exact(&mut buf[..n])?;
        if buf[..n].iter().any(|&b| b != 0) {
            bail!("non-zero byte found in ISO9660 directory padding at byte {offset}");
        }
        remaining -= n as u64;
    }
    Ok(())
}

fn write_zeroes(file: &mut File, len: u64) -> Result<()> {
    let buf = [0u8; 4096];
    let mut remaining = len;
    while remaining > 0 {
        let n = remaining.min(buf.len() as u64) as usize;
        file.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    Ok(())
}

/// Case-insensitive ISO9660 PVD name compare. The `;1` version suffix
/// is optional per spec; XP SP3 master ISOs omit it on every record we
/// inspected, while the WINNT_SIF_NAME constant historically included
/// it. Normalise both sides by stripping a trailing `;1` before
/// comparing so callers can pass either form.
fn iso_name_eq(actual: &[u8], expected: &[u8]) -> bool {
    let actual_base = actual.strip_suffix(b";1").unwrap_or(actual);
    let expected_base = expected.strip_suffix(b";1").unwrap_or(expected);
    actual_base.eq_ignore_ascii_case(expected_base)
}

fn align_up(value: u64, alignment: u64) -> u64 {
    debug_assert!(alignment.is_power_of_two());
    (value + alignment - 1) & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn injects_winnt_sif_into_final_directory_sector_padding() {
        let path = temp_iso_path("bootsmith-ntxp-iso-inject");
        fs::write(&path, minimal_iso(false)).unwrap();

        inject_winnt_sif(&path, b"[Data]\r\nUnattendedInstall=\"Yes\"\r\n").unwrap();

        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len() as u64, 22 * SECTOR_SIZE);
        assert_eq!(
            read_733(&bytes[(PVD_OFFSET + 80) as usize..(PVD_OFFSET + 88) as usize]).unwrap(),
            22
        );

        let root = parse_directory_record(
            &bytes[PVD_OFFSET as usize..(PVD_OFFSET + SECTOR_SIZE) as usize],
            ROOT_RECORD_OFFSET_IN_PVD,
            PVD_OFFSET,
        )
        .unwrap();
        let i386 = records_from_bytes(&bytes, &root)
            .into_iter()
            .find(|r| r.name == b"I386")
            .unwrap();
        // The unified sorted-append path packs records tightly rather
        // than padding to the next sector boundary. minimal_iso(false)
        // starts with 3 records totalling 112 bytes; adding WINNT.SIF
        // (44 bytes) puts the directory at 156 bytes.
        assert_eq!(i386.data_len, 156);

        let records = records_from_bytes(&bytes, &i386);
        let winnt = records
            .into_iter()
            .find(|r| r.name.eq_ignore_ascii_case(WINNT_SIF_NAME))
            .unwrap();
        assert_eq!(winnt.extent_lba, 21);
        assert_eq!(winnt.data_len, 33);
        assert_eq!(
            &bytes[21 * SECTOR_SIZE as usize..21 * SECTOR_SIZE as usize + 33],
            b"[Data]\r\nUnattendedInstall=\"Yes\"\r\n"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_existing_winnt_sif() {
        let path = temp_iso_path("bootsmith-ntxp-iso-existing");
        fs::write(&path, minimal_iso(true)).unwrap();

        let err = inject_winnt_sif(&path, b"x").unwrap_err().to_string();
        assert!(err.contains("already exists"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn append_file_to_i386_writes_record_and_data() {
        let path = temp_iso_path("bootsmith-append-file");
        fs::write(&path, minimal_iso(false)).unwrap();

        append_file_to_i386(&path, b"IASTOR.SYS;1", b"FAKE_DRIVER").unwrap();

        let bytes = fs::read(&path).unwrap();
        let root = parse_directory_record(
            &bytes[PVD_OFFSET as usize..(PVD_OFFSET + SECTOR_SIZE) as usize],
            ROOT_RECORD_OFFSET_IN_PVD,
            PVD_OFFSET,
        )
        .unwrap();
        let i386 = records_from_bytes(&bytes, &root)
            .into_iter()
            .find(|r| r.name == b"I386")
            .unwrap();
        let records = records_from_bytes(&bytes, &i386);
        let iastor = records
            .iter()
            .find(|r| iso_name_eq(&r.name, b"IASTOR.SYS;1"))
            .expect("IASTOR.SYS record present");
        let data = &bytes[iastor.extent_lba as usize * SECTOR_SIZE as usize
            ..iastor.extent_lba as usize * SECTOR_SIZE as usize + iastor.data_len as usize];
        assert_eq!(data, b"FAKE_DRIVER");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn append_file_to_i386_supports_multiple_calls() {
        // Multiple appends in sequence: each call sees the directory
        // state left by the previous one. This is the scenario the
        // slipstream pipeline needs -- iaStor.sys + iaAHCI.inf +
        // iaAHCI.cat + iaStor.inf + iaStor.cat in one pass.
        let path = temp_iso_path("bootsmith-append-multi");
        fs::write(&path, minimal_iso(false)).unwrap();

        append_file_to_i386(&path, b"A.SYS;1", b"AAA").unwrap();
        append_file_to_i386(&path, b"B.INF;1", b"BBBB").unwrap();
        append_file_to_i386(&path, b"C.CAT;1", b"CCCCC").unwrap();

        let bytes = fs::read(&path).unwrap();
        let root = parse_directory_record(
            &bytes[PVD_OFFSET as usize..(PVD_OFFSET + SECTOR_SIZE) as usize],
            ROOT_RECORD_OFFSET_IN_PVD,
            PVD_OFFSET,
        )
        .unwrap();
        let i386 = records_from_bytes(&bytes, &root)
            .into_iter()
            .find(|r| r.name == b"I386")
            .unwrap();
        let records = records_from_bytes(&bytes, &i386);

        for (name, expected) in [
            (b"A.SYS;1".as_ref(), b"AAA".as_ref()),
            (b"B.INF;1".as_ref(), b"BBBB".as_ref()),
            (b"C.CAT;1".as_ref(), b"CCCCC".as_ref()),
        ] {
            let r = records
                .iter()
                .find(|r| iso_name_eq(&r.name, name))
                .unwrap_or_else(|| panic!("record {:?} missing", String::from_utf8_lossy(name)));
            let off = r.extent_lba as usize * SECTOR_SIZE as usize;
            assert_eq!(&bytes[off..off + r.data_len as usize], expected);
        }

        let _ = fs::remove_file(path);
    }

    #[test]
    fn append_file_to_i386_keeps_directory_sorted() {
        let path = temp_iso_path("bootsmith-append-sorted");
        fs::write(&path, minimal_iso(false)).unwrap();

        append_file_to_i386(&path, b"IASTOR.SYS;1", b"DRIVER").unwrap();
        append_file_to_i386(&path, b"IAAHCI.INF;1", b"INF").unwrap();

        let bytes = fs::read(&path).unwrap();
        let root = parse_directory_record(
            &bytes[PVD_OFFSET as usize..(PVD_OFFSET + SECTOR_SIZE) as usize],
            ROOT_RECORD_OFFSET_IN_PVD,
            PVD_OFFSET,
        )
        .unwrap();
        let i386 = records_from_bytes(&bytes, &root)
            .into_iter()
            .find(|r| r.name == b"I386")
            .unwrap();
        let names: Vec<Vec<u8>> = records_from_bytes(&bytes, &i386)
            .into_iter()
            .skip(2)
            .map(|r| r.name)
            .collect();
        assert_eq!(
            names,
            vec![
                b"AAAA.TXT;1".to_vec(),
                b"IAAHCI.INF;1".to_vec(),
                b"IASTOR.SYS;1".to_vec()
            ]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn append_file_to_root_writes_record_at_volume_root() {
        // Drop a file at the ISO root and verify it shows up in the
        // root directory listing (and NOT under /I386). The AHCI
        // slipstream uses this to duplicate iaStor.sys at F:\ so XP
        // GUI-mode PnP finds it on the first prompt.
        let path = temp_iso_path("bootsmith-append-root");
        fs::write(&path, minimal_iso(false)).unwrap();

        append_file_to_root(&path, b"IASTOR.SYS;1", b"FAKE_DRIVER").unwrap();

        let bytes = fs::read(&path).unwrap();
        let root = parse_directory_record(
            &bytes[PVD_OFFSET as usize..(PVD_OFFSET + SECTOR_SIZE) as usize],
            ROOT_RECORD_OFFSET_IN_PVD,
            PVD_OFFSET,
        )
        .unwrap();
        // File visible in root.
        let root_records = records_from_bytes(&bytes, &root);
        let iastor = root_records
            .iter()
            .find(|r| iso_name_eq(&r.name, b"IASTOR.SYS;1"))
            .expect("IASTOR.SYS present in root");
        let off = iastor.extent_lba as usize * SECTOR_SIZE as usize;
        assert_eq!(
            &bytes[off..off + iastor.data_len as usize],
            b"FAKE_DRIVER"
        );
        // File NOT also leaked into /I386.
        let i386 = root_records
            .iter()
            .find(|r| r.name == b"I386")
            .expect("I386 still present in root");
        let i386_records = records_from_bytes(&bytes, i386);
        assert!(
            !i386_records
                .iter()
                .any(|r| iso_name_eq(&r.name, b"IASTOR.SYS;1")),
            "root append should not also write to I386"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn append_file_to_root_rejects_duplicates() {
        let path = temp_iso_path("bootsmith-append-root-dup");
        fs::write(&path, minimal_iso(false)).unwrap();

        append_file_to_root(&path, b"DUP.SYS;1", b"first").unwrap();
        let err = append_file_to_root(&path, b"DUP.SYS;1", b"second")
            .unwrap_err()
            .to_string();
        assert!(err.contains("already exists"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn append_file_to_i386_rejects_duplicates() {
        let path = temp_iso_path("bootsmith-append-dup");
        fs::write(&path, minimal_iso(false)).unwrap();

        append_file_to_i386(&path, b"DUP.SYS;1", b"first").unwrap();
        let err = append_file_to_i386(&path, b"DUP.SYS;1", b"second")
            .unwrap_err()
            .to_string();
        assert!(err.contains("already exists"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn replace_file_in_i386_relocates_to_new_extent() {
        // minimal_iso has AAAA.TXT as a placeholder. Replace it and
        // verify the directory record now points at a new LBA with new
        // contents, while the old extent is left in place but
        // unreferenced.
        let path = temp_iso_path("bootsmith-replace");
        fs::write(&path, minimal_iso(false)).unwrap();

        replace_file_in_i386(&path, b"AAAA.TXT;1", b"REPLACED_CONTENT").unwrap();

        let bytes = fs::read(&path).unwrap();
        let root = parse_directory_record(
            &bytes[PVD_OFFSET as usize..(PVD_OFFSET + SECTOR_SIZE) as usize],
            ROOT_RECORD_OFFSET_IN_PVD,
            PVD_OFFSET,
        )
        .unwrap();
        let i386 = records_from_bytes(&bytes, &root)
            .into_iter()
            .find(|r| r.name == b"I386")
            .unwrap();
        let records = records_from_bytes(&bytes, &i386);
        let txt = records
            .iter()
            .find(|r| iso_name_eq(&r.name, b"AAAA.TXT;1"))
            .unwrap();
        // The record's extent_lba should now point past the original
        // ISO end (which was 21 sectors).
        assert!(txt.extent_lba >= 21);
        assert_eq!(txt.data_len as usize, b"REPLACED_CONTENT".len());
        let off = txt.extent_lba as usize * SECTOR_SIZE as usize;
        assert_eq!(
            &bytes[off..off + b"REPLACED_CONTENT".len()],
            b"REPLACED_CONTENT"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn replace_file_in_i386_handles_growth_past_original_size() {
        // The whole reason replace exists: TXTSETUP.SIF starts at ~750KB
        // and grows by a few KB after the slipstream patch. Replace
        // must handle "new content larger than old extent" cleanly.
        let path = temp_iso_path("bootsmith-replace-grow");
        fs::write(&path, minimal_iso(false)).unwrap();

        let big = vec![b'X'; 5_000];
        replace_file_in_i386(&path, b"AAAA.TXT;1", &big).unwrap();

        let bytes = fs::read(&path).unwrap();
        let root = parse_directory_record(
            &bytes[PVD_OFFSET as usize..(PVD_OFFSET + SECTOR_SIZE) as usize],
            ROOT_RECORD_OFFSET_IN_PVD,
            PVD_OFFSET,
        )
        .unwrap();
        let i386 = records_from_bytes(&bytes, &root)
            .into_iter()
            .find(|r| r.name == b"I386")
            .unwrap();
        let txt = records_from_bytes(&bytes, &i386)
            .into_iter()
            .find(|r| iso_name_eq(&r.name, b"AAAA.TXT;1"))
            .unwrap();
        assert_eq!(txt.data_len, 5_000);
        let off = txt.extent_lba as usize * SECTOR_SIZE as usize;
        assert_eq!(&bytes[off..off + 5_000], big.as_slice());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn replace_file_in_i386_rejects_missing() {
        let path = temp_iso_path("bootsmith-replace-missing");
        fs::write(&path, minimal_iso(false)).unwrap();
        let err = replace_file_in_i386(&path, b"NOSUCH.SYS;1", b"x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not found"));
        let _ = fs::remove_file(path);
    }

    #[test]
    #[ignore = "set BOOTSMITH_XP_ISO_SMOKE=/path/to/xp.iso to run against real media"]
    fn real_xp_iso_smoke() {
        let src = std::env::var_os("BOOTSMITH_XP_ISO_SMOKE")
            .expect("BOOTSMITH_XP_ISO_SMOKE must point to an XP ISO");
        let path = temp_iso_path("bootsmith-ntxp-real-iso");
        fs::copy(&src, &path).unwrap();

        let sif = b"[Data]\r\nAutoPartition=0\r\nUnattendedInstall=\"Yes\"\r\n";
        inject_winnt_sif(&path, sif).unwrap();

        let mut file = File::open(&path).unwrap();
        let pvd = read_sector(&mut file, PVD_LBA).unwrap();
        let root = parse_directory_record(&pvd, ROOT_RECORD_OFFSET_IN_PVD, PVD_OFFSET).unwrap();
        let i386 = find_child_record(&mut file, &root, b"I386")
            .unwrap()
            .unwrap();
        let winnt = find_child_record(&mut file, &i386, WINNT_SIF_NAME)
            .unwrap()
            .unwrap();

        let mut actual = vec![0u8; winnt.data_len as usize];
        file.seek(SeekFrom::Start(u64::from(winnt.extent_lba) * SECTOR_SIZE))
            .unwrap();
        file.read_exact(&mut actual).unwrap();
        assert_eq!(actual, sif);

        if std::env::var_os("BOOTSMITH_KEEP_SMOKE_ISO").is_some() {
            eprintln!("{}", path.display());
        } else {
            let _ = fs::remove_file(path);
        }
    }

    fn records_from_bytes(bytes: &[u8], directory: &DirectoryRecord) -> Vec<DirectoryRecord> {
        let start = directory.extent_lba as usize * SECTOR_SIZE as usize;
        let end = start + directory.data_len as usize;
        let mut records = Vec::new();
        let mut pos = start;
        while pos < end {
            if bytes[pos] == 0 {
                pos = align_up((pos + 1) as u64, SECTOR_SIZE) as usize;
                continue;
            }
            let len = bytes[pos] as usize;
            records.push(parse_directory_record(&bytes[pos..pos + len], 0, pos as u64).unwrap());
            pos += len;
        }
        records
    }

    fn minimal_iso(with_winnt: bool) -> Vec<u8> {
        let mut iso = vec![0u8; 21 * SECTOR_SIZE as usize];
        let i386_len = if with_winnt { 156 } else { 112 };
        let pvd = PVD_OFFSET as usize;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 6] = 1;
        write_733(&mut iso[pvd + 80..pvd + 88], 21);

        let root = dir_record(b"\0", 20, 140, true);
        iso[pvd + ROOT_RECORD_OFFSET_IN_PVD..pvd + ROOT_RECORD_OFFSET_IN_PVD + root.len()]
            .copy_from_slice(&root);

        let root_dir = 20 * SECTOR_SIZE as usize;
        let dot = dir_record(b"\0", 20, 140, true);
        let dotdot = dir_record(b"\x01", 20, 140, true);
        let i386 = dir_record(b"I386", 19, i386_len, true);
        iso[root_dir..root_dir + dot.len()].copy_from_slice(&dot);
        iso[root_dir + dot.len()..root_dir + dot.len() + dotdot.len()].copy_from_slice(&dotdot);
        iso[root_dir + dot.len() + dotdot.len()..root_dir + dot.len() + dotdot.len() + i386.len()]
            .copy_from_slice(&i386);

        let i386_dir = 19 * SECTOR_SIZE as usize;
        let dot = dir_record(b"\0", 19, i386_len, true);
        let dotdot = dir_record(b"\x01", 20, 140, true);
        let marker = dir_record(b"AAAA.TXT;1", 18, 4, false);
        let mut pos = i386_dir;
        for record in [&dot, &dotdot, &marker] {
            iso[pos..pos + record.len()].copy_from_slice(record);
            pos += record.len();
        }
        if with_winnt {
            let winnt = dir_record(WINNT_SIF_NAME, 17, 1, false);
            iso[pos..pos + winnt.len()].copy_from_slice(&winnt);
        }
        iso
    }

    fn dir_record(name: &[u8], extent_lba: u32, data_len: u32, is_dir: bool) -> Vec<u8> {
        let len = 33 + name.len() + ((33 + name.len()) % 2);
        let mut record = vec![0u8; len];
        record[0] = len as u8;
        write_733(&mut record[2..10], extent_lba);
        write_733(&mut record[10..18], data_len);
        record[18] = 126;
        record[19] = 5;
        record[20] = 21;
        record[25] = if is_dir { DIRECTORY_FLAG } else { 0 };
        write_723(&mut record[28..32], 1);
        record[32] = name.len() as u8;
        record[33..33 + name.len()].copy_from_slice(name);
        record
    }

    fn temp_iso_path(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}.iso"))
    }
}
