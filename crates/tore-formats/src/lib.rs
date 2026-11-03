//! Bounded readers for user-owned Fighters Anthology menu and theater resources.
//! No executable resource is ever executed. See docs/formats/menu.md and theater.md.
pub mod aircraft;
pub mod carrier;
pub mod chat;
mod dcl;
pub mod esa;
pub mod executable;
pub mod font;
pub mod hud;
pub mod missile_data;
pub mod missile_motors;
pub mod mission;
pub mod mission_text;
pub mod module;
pub mod music;
pub mod parked_aircraft;
pub mod pcm;
mod pic;
pub mod quick_template;
pub mod radio;
pub mod shape;
pub mod static_object;
pub mod strip;
pub mod surface_set;
pub mod surface_unit;
pub mod text;
pub mod theater;
pub mod ui;
pub mod weapons;
pub mod weather;
pub use pic::Pic;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::Mutex,
};
pub use ui::{Button, activity_buttons};

pub type Result<T> = io::Result<T>;
pub(crate) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub(crate) fn slice(data: &[u8], start: usize, size: usize) -> Result<&[u8]> {
    data.get(
        start
            ..start
                .checked_add(size)
                .ok_or_else(|| invalid("offset overflow"))?,
    )
    .ok_or_else(|| invalid("resource truncated"))
}
pub(crate) fn u16_at(data: &[u8], at: usize) -> Result<usize> {
    Ok(u16::from_le_bytes(slice(data, at, 2)?.try_into().unwrap()) as usize)
}
pub(crate) fn u32_at(data: &[u8], at: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(slice(data, at, 4)?.try_into().unwrap()) as usize)
}

#[derive(Debug)]
pub struct Entry {
    pub name: String,
    pub flag: u8,
    pub offset: usize,
    pub size: usize,
}
pub struct Archive {
    source: Source,
    /// File offset the archive starts at; nonzero when it is stored inside a container.
    base: u64,
    pub entries: BTreeMap<String, Entry>,
}
enum Source {
    Memory(Vec<u8>),
    File(Mutex<File>),
}
impl Archive {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        let entries = Self::directory(&data, data.len())?;
        Ok(Self {
            source: Source::Memory(data),
            base: 0,
            entries,
        })
    }
    /// Read just the directory; resource reads seek directly into the archive.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Self::from_file(file, 0, len)
    }
    /// Serve an archive stored inside a larger file, such as a LIB inside `SETUP.ESA`.
    /// `len` is the archive's own length; every read is offset by `offset`.
    pub fn open_at(path: impl AsRef<Path>, offset: u64, len: u64) -> Result<Self> {
        let file = File::open(path)?;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| invalid("archive window overflows the file"))?;
        if end > file.metadata()?.len() {
            return Err(invalid("archive window lies outside the file"));
        }
        Self::from_file(file, offset, len)
    }
    fn from_file(mut file: File, base: u64, len: u64) -> Result<Self> {
        let file_size =
            usize::try_from(len).map_err(|_| invalid("archive too large for this host"))?;
        file.seek(SeekFrom::Start(base))?;
        let mut header = [0; 7];
        file.read_exact(&mut header)?;
        if &header[..5] != b"EALIB" {
            return Err(invalid("not an EALIB archive"));
        }
        let size = 7 + (u16_at(&header, 5)? + 1) * 18;
        if size > file_size {
            return Err(invalid("archive directory exceeds file"));
        }
        let mut directory = vec![0; size];
        directory[..7].copy_from_slice(&header);
        file.read_exact(&mut directory[7..])?;
        let entries = Self::directory(&directory, file_size)?;
        Ok(Self {
            source: Source::File(Mutex::new(file)),
            base,
            entries,
        })
    }
    fn directory(data: &[u8], file_size: usize) -> Result<BTreeMap<String, Entry>> {
        if slice(data, 0, 5)? != b"EALIB" {
            return Err(invalid("not an EALIB archive"));
        }
        let count = u16_at(data, 5)?;
        let end = 7 + (count + 1) * 18;
        slice(data, 0, end)?;
        let sentinel = 7 + count * 18;
        if data[sentinel..sentinel + 14].iter().any(|b| *b != 0)
            || u32_at(data, sentinel + 14)? != file_size
        {
            return Err(invalid("invalid archive sentinel"));
        }
        let mut entries = BTreeMap::new();
        for index in 0..count {
            let at = 7 + index * 18;
            let raw = &data[at..at + 13];
            let name = std::str::from_utf8(raw.split(|b| *b == 0).next().unwrap())
                .map_err(|_| invalid("non-ASCII archive name"))?
                .to_ascii_uppercase();
            if name.is_empty() || name.contains(['/', '\\']) || name == ".." {
                return Err(invalid("unsafe archive name"));
            }
            let offset = u32_at(data, at + 14)?;
            let next = u32_at(data, at + 32)?;
            if offset < end || next < offset || next > file_size {
                return Err(invalid("archive entry outside data"));
            }
            let flag = data[at + 13];
            if !matches!(flag, 0 | 4) {
                return Err(invalid("unsupported archive compression flag"));
            }
            entries.insert(
                name.clone(),
                Entry {
                    name,
                    flag,
                    offset,
                    size: next - offset,
                },
            );
        }
        Ok(entries)
    }
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        self.read_with_limit(name, 16 * 1024 * 1024)
    }
    /// Generic extraction can choose a larger cap; the menu retains 16 MiB.
    pub fn read_with_limit(&self, name: &str, limit: usize) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .get(&name.to_ascii_uppercase())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("missing resource {name}"))
            })?;
        let stored_limit = if entry.flag == 0 {
            limit
        } else {
            limit.saturating_mul(2).saturating_add(4)
        };
        if entry.size > stored_limit {
            return Err(invalid("resource exceeds configured byte limit"));
        }
        let data = match &self.source {
            Source::Memory(data) => slice(data, entry.offset, entry.size)?.to_vec(),
            Source::File(file) => {
                let mut file = file
                    .lock()
                    .map_err(|_| io::Error::other("archive file lock poisoned"))?;
                file.seek(SeekFrom::Start(self.base + entry.offset as u64))?;
                let mut data = vec![0; entry.size];
                file.read_exact(&mut data)?;
                data
            }
        };
        if entry.flag == 0 {
            return Ok(data);
        }
        dcl::explode(
            slice(&data, 4, data.len().saturating_sub(4))?,
            u32_at(&data, 0)?,
            limit,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_archives_fail_without_panicking() {
        for length in 0..80 {
            assert!(Archive::parse(vec![0; length]).is_err());
        }
    }
    #[test]
    fn stored_entry_and_sentinel_boundaries() {
        let mut data = b"EALIB\x01\x00".to_vec();
        data.extend_from_slice(b"TEST.TXT\0\0\0\0\0\0");
        data.extend_from_slice(&43_u32.to_le_bytes());
        data.extend_from_slice(&[0; 14]);
        data.extend_from_slice(&46_u32.to_le_bytes());
        data.extend_from_slice(b"abc");
        let archive = Archive::parse(data.clone()).unwrap();
        assert_eq!(archive.read("test.txt").unwrap(), b"abc");
        data[39] = 45;
        assert!(Archive::parse(data).is_err());
    }
    #[test]
    fn open_at_reads_an_archive_stored_inside_a_larger_file() {
        let mut archive = b"EALIB\x01\x00".to_vec();
        archive.extend_from_slice(b"TEST.TXT\0\0\0\0\0\0");
        archive.extend_from_slice(&43_u32.to_le_bytes());
        archive.extend_from_slice(&[0; 14]);
        archive.extend_from_slice(&46_u32.to_le_bytes());
        archive.extend_from_slice(b"abc");
        let base = 17_u64;
        let mut file = vec![0xcd; base as usize];
        file.extend_from_slice(&archive);
        file.extend_from_slice(b"trailing bytes");
        let path = std::env::temp_dir().join(format!("tore-open-at-{}.bin", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        let opened = Archive::open_at(&path, base, archive.len() as u64).unwrap();
        assert_eq!(opened.read("test.txt").unwrap(), b"abc");
        // The sentinel compares against the archive length, not the whole file.
        assert!(Archive::open_at(&path, base, archive.len() as u64 + 1).is_err());
        assert!(Archive::open_at(&path, base, file.len() as u64).is_err());
        assert!(Archive::open(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}

pub mod flight_model;

pub mod ejection;
