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

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(iso_path)
        .with_context(|| format!("open {} for WINNT.SIF injection", iso_path.display()))?;

    let iso_len = file
        .metadata()
        .with_context(|| format!("stat {}", iso_path.display()))?
        .len();
    if iso_len < (PVD_LBA + 1) * SECTOR_SIZE {
        bail!("{} is too small to be an ISO9660 image", iso_path.display());
    }

    let pvd = read_sector(&mut file, PVD_LBA).context("read ISO9660 primary volume descriptor")?;
    validate_pvd(&pvd)?;
    let root = parse_directory_record(&pvd, ROOT_RECORD_OFFSET_IN_PVD, PVD_OFFSET)
        .context("parse ISO9660 root directory record")?;
    if root.flags & DIRECTORY_FLAG == 0 {
        bail!("ISO9660 root record is not a directory");
    }

    let i386 = find_child_record(&mut file, &root, b"I386")
        .context("locate ISO9660 /I386 directory")?
        .ok_or_else(|| anyhow::anyhow!("ISO9660 /I386 directory not found"))?;
    if i386.flags & DIRECTORY_FLAG == 0 {
        bail!("ISO9660 /I386 is not a directory");
    }
    let i386_records = read_directory_records(&mut file, &i386)?;
    if i386_records
        .iter()
        .any(|record| iso_name_eq(&record.name, WINNT_SIF_NAME))
    {
        bail!("ISO9660 /I386/WINNT.SIF already exists");
    }

    let i386_start = u64::from(i386.extent_lba) * SECTOR_SIZE;
    let old_i386_len = u64::from(i386.data_len);
    let last_record_end = i386_records
        .iter()
        .map(|record| record.offset + u64::from(record.len) - i386_start)
        .max()
        .unwrap_or(0);
    if last_record_end != old_i386_len {
        bail!(
            "ISO9660 /I386 directory has trailing zero gap inside its declared size; refusing append-only WINNT.SIF patch"
        );
    }

    let new_i386_len = align_up(old_i386_len, SECTOR_SIZE);
    if new_i386_len == old_i386_len {
        bail!("ISO9660 /I386 directory has no final-sector padding for WINNT.SIF");
    }

    let record = build_file_record(
        WINNT_SIF_NAME,
        u32::try_from(align_up(iso_len, SECTOR_SIZE) / SECTOR_SIZE)
            .context("patched ISO would exceed 32-bit ISO9660 LBA range")?,
        u32::try_from(contents.len()).context("WINNT.SIF is too large for ISO9660")?,
    )?;
    let record_offset = i386_start + old_i386_len;
    let padding_len = new_i386_len - old_i386_len;
    if record.len() as u64 > padding_len {
        bail!(
            "ISO9660 /I386 final-sector padding is {padding_len} bytes; WINNT.SIF record needs {} bytes",
            record.len()
        );
    }
    ensure_zero_range(&mut file, record_offset, padding_len)
        .context("verify /I386 final-sector padding is zero-filled")?;

    let file_data_offset = align_up(iso_len, SECTOR_SIZE);
    let final_len = align_up(file_data_offset + contents.len() as u64, SECTOR_SIZE);
    let final_sectors = final_len / SECTOR_SIZE;
    let new_i386_len_u32 = u32::try_from(new_i386_len)
        .context("ISO9660 /I386 directory is too large for this patcher")?;
    let final_sectors_u32 =
        u32::try_from(final_sectors).context("patched ISO exceeds 32-bit ISO9660 volume size")?;

    file.seek(SeekFrom::Start(record_offset))?;
    file.write_all(&record)?;

    write_733_at(&mut file, i386.offset + 10, new_i386_len_u32)
        .context("update root /I386 directory record size")?;
    write_733_at(&mut file, i386_start + 10, new_i386_len_u32)
        .context("update /I386 self directory record size")?;
    write_733_at(
        &mut file,
        VOLUME_SPACE_SIZE_OFFSET_IN_PVD,
        final_sectors_u32,
    )
    .context("update ISO9660 volume size")?;

    file.seek(SeekFrom::Start(iso_len))?;
    if file_data_offset > iso_len {
        write_zeroes(&mut file, file_data_offset - iso_len)?;
    }
    file.write_all(contents)?;
    if final_len > file_data_offset + contents.len() as u64 {
        write_zeroes(
            &mut file,
            final_len - file_data_offset - contents.len() as u64,
        )?;
    }
    file.set_len(final_len)?;
    file.sync_all()?;
    Ok(())
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

fn iso_name_eq(actual: &[u8], expected: &[u8]) -> bool {
    actual.eq_ignore_ascii_case(expected)
        || actual
            .strip_suffix(b";1")
            .is_some_and(|base| base.eq_ignore_ascii_case(expected))
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
        let path = temp_iso_path("usbwin-ntxp-iso-inject");
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
        assert_eq!(i386.data_len, SECTOR_SIZE as u32);

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
        let path = temp_iso_path("usbwin-ntxp-iso-existing");
        fs::write(&path, minimal_iso(true)).unwrap();

        let err = inject_winnt_sif(&path, b"x").unwrap_err().to_string();
        assert!(err.contains("already exists"));

        let _ = fs::remove_file(path);
    }

    #[test]
    #[ignore = "set USBWIN_XP_ISO_SMOKE=/path/to/xp.iso to run against real media"]
    fn real_xp_iso_smoke() {
        let src = std::env::var_os("USBWIN_XP_ISO_SMOKE")
            .expect("USBWIN_XP_ISO_SMOKE must point to an XP ISO");
        let path = temp_iso_path("usbwin-ntxp-real-iso");
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

        if std::env::var_os("USBWIN_KEEP_SMOKE_ISO").is_some() {
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
