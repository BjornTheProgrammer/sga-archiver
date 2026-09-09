//! Metadata-only view of an archive, with members read on demand.
//!
//! [`Archive`](crate::Archive) loads every member's bytes as it parses, which
//! is what the byte-exact writer needs but is the wrong shape for listing a
//! multi-gigabyte base-game archive or pulling one member out of it. This
//! module parses only the header, the tables and the string blob, keeps each
//! member's offsets, and seeks to a member's bytes when a caller asks.

use std::io::{BufRead, Read, Seek, SeekFrom};

use anyhow::{Context, Result, anyhow};
use binrw::BinRead;
use flate2::Crc;
use thiserror::Error;

use crate::archive::{block_sha1, decode_stored};
use crate::entries::{
    FileEncryptionType, FileStorageType, FileVerificationType, SgaFileEntry, SgaFolderEntry,
    SgaHeader, SgaToC,
};

/// A member is stored encrypted, and this crate does not decrypt.
///
/// Returned (inside `anyhow::Error`) wherever decoded bytes were asked for, so
/// a caller never receives ciphertext believing it to be the member.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{path} is stored with {encryption:?} encryption, which this crate cannot decrypt")]
pub struct EncryptedMember {
    pub path: String,
    pub encryption: FileEncryptionType,
}

/// Where one member's bytes live and how to decode them. No data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFile {
    pub name: String,
    /// Absolute offset of the stored bytes within the archive file.
    pub data_offset: u64,
    /// Offset relative to the data blob, as written in the entry. This is the
    /// `data_order` a [`FileEntry`](crate::FileEntry) carries.
    pub data_order: u64,
    pub stored_size: u32,
    pub uncompressed_size: u32,
    pub storage_type: FileStorageType,
    pub encryption_type: FileEncryptionType,
    pub verification_type: FileVerificationType,
    pub crc: u32,
    /// Offset of this member's block hashes within the hash blob.
    pub hash_offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFolder {
    pub name: String,
    pub folders: Vec<IndexFolder>,
    pub files: Vec<IndexFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexToc {
    pub alias: String,
    pub name: String,
    pub root: IndexFolder,
}

/// Everything an archive says about itself short of member bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchiveIndex {
    pub header: SgaHeader,
    pub tocs: Vec<IndexToc>,
    /// The per-block hash table, empty when the archive has none.
    pub hash_blob: Vec<u8>,
}

impl ArchiveIndex {
    pub fn read<R: Read + BufRead + Seek>(reader: &mut R) -> Result<ArchiveIndex> {
        let header = SgaHeader::parse(reader).map_err(|e| anyhow!(e.to_string()))?;
        let version = header.version;

        reader.seek(SeekFrom::Start(
            header.header_blob_offset + header.toc_data_offset as u64,
        ))?;
        let mut toc_entries = Vec::with_capacity(header.toc_data_count as usize);
        for _ in 0..header.toc_data_count {
            toc_entries.push(SgaToC::read_le_args(reader, (version,))?);
        }

        reader.seek(SeekFrom::Start(
            header.header_blob_offset + header.folder_data_offset as u64,
        ))?;
        let mut folder_entries = Vec::with_capacity(header.folder_data_count as usize);
        for _ in 0..header.folder_data_count {
            folder_entries.push(SgaFolderEntry::read_le_args(reader, (version,))?);
        }

        reader.seek(SeekFrom::Start(
            header.header_blob_offset + header.file_data_offset as u64,
        ))?;
        let mut file_entries = Vec::with_capacity(header.file_data_count as usize);
        for _ in 0..header.file_data_count {
            file_entries.push(SgaFileEntry::read_le_args(reader, (version,))?);
        }

        reader.seek(SeekFrom::Start(
            header.header_blob_offset + header.string_offset as u64,
        ))?;
        let mut string_blob = vec![0u8; header.string_length as usize];
        reader.read_exact(&mut string_blob)?;

        let hash_blob = if version >= 8 && header.file_hash_length > 0 {
            reader.seek(SeekFrom::Start(
                header.header_blob_offset + header.file_hash_offset as u64,
            ))?;
            let mut blob = vec![0u8; header.file_hash_length as usize];
            reader.read_exact(&mut blob)?;
            blob
        } else {
            Vec::new()
        };

        let mut tocs = Vec::with_capacity(toc_entries.len());
        for te in &toc_entries {
            let root = index_folder(
                &header,
                &folder_entries,
                &file_entries,
                &string_blob,
                te.folder_root_index as usize,
            )?;
            tocs.push(IndexToc {
                alias: trim_fixed(&te.alias),
                name: trim_fixed(&te.name),
                root,
            });
        }

        Ok(ArchiveIndex {
            header,
            tocs,
            hash_blob,
        })
    }

    /// Every member, paired with the alias of its TOC and its `\`-separated
    /// path within that TOC, in table order.
    pub fn files(&self) -> impl Iterator<Item = (&IndexToc, String, &IndexFile)> + '_ {
        self.tocs.iter().flat_map(|toc| {
            let mut out = Vec::new();
            collect_files(&toc.root, String::new(), &mut out);
            out.into_iter().map(move |(path, file)| (toc, path, file))
        })
    }

    pub fn file_count(&self) -> usize {
        self.tocs.iter().map(|toc| count_files(&toc.root)).sum()
    }

    pub fn folder_count(&self) -> usize {
        self.tocs.iter().map(|toc| count_folders(&toc.root)).sum()
    }

    /// The member at `rel`, if the archive holds one.
    ///
    /// Same contract as [`Archive::file`](crate::Archive::file): `/` or `\`
    /// separators, exact case, every TOC searched, first match wins.
    pub fn file(&self, rel: &str) -> Option<(&IndexToc, &IndexFile)> {
        let components = rel.split(['/', '\\']).collect::<Vec<_>>();
        let (name, dirs) = components.split_last()?;
        for toc in &self.tocs {
            let mut folder = &toc.root;
            let mut reached = true;
            for dir in dirs {
                match folder.folders.iter().find(|f| f.name == *dir) {
                    Some(child) => folder = child,
                    None => {
                        reached = false;
                        break;
                    }
                }
            }
            if reached && let Some(file) = folder.files.iter().find(|f| f.name == *name) {
                return Some((toc, file));
            }
        }
        None
    }

    /// The SHA1 block hashes recorded for `file`, or `None` when the archive
    /// carries none for it.
    pub fn recorded_block_hashes(&self, file: &IndexFile) -> Option<&[u8]> {
        if file.verification_type != FileVerificationType::SHA1Blocks || self.hash_blob.is_empty() {
            return None;
        }
        let blocks = block_count(file.stored_size as usize, self.header.block_size as usize);
        let start = file.hash_offset as usize;
        let end = start.checked_add(blocks * 20)?;
        self.hash_blob.get(start..end)
    }
}

/// An open archive whose members are read as they are asked for.
pub struct ArchiveReader<R> {
    index: ArchiveIndex,
    reader: R,
}

/// One member that failed a check during [`ArchiveReader::verify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub toc: String,
    pub path: String,
    pub check: &'static str,
}

/// Outcome of checking every member's stored bytes against what the archive
/// recorded for them.
///
/// This is a data-integrity check, not a signature or authenticity check: the
/// header signature is neither parsed nor validated.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Verification {
    pub checked_files: usize,
    /// Members whose entry has a nonzero CRC that the stored bytes do not match.
    pub crc_mismatches: Vec<Mismatch>,
    /// Members with SHA1 block hashes that the stored bytes do not match.
    pub sha1_mismatches: Vec<Mismatch>,
    /// Members that declared block verification the archive carries no hashes
    /// for, so nothing could be checked.
    pub unhashed_files: usize,
}

impl Verification {
    pub fn verified(&self) -> bool {
        self.crc_mismatches.is_empty() && self.sha1_mismatches.is_empty()
    }
}

impl<R: Read + BufRead + Seek> ArchiveReader<R> {
    pub fn new(mut reader: R) -> Result<Self> {
        let index = ArchiveIndex::read(&mut reader)?;
        Ok(ArchiveReader { index, reader })
    }

    pub fn index(&self) -> &ArchiveIndex {
        &self.index
    }

    pub fn into_index(self) -> ArchiveIndex {
        self.index
    }

    /// The bytes exactly as stored: compressed and, if applicable, encrypted.
    pub fn read_stored(&mut self, file: &IndexFile) -> Result<Vec<u8>> {
        self.reader.seek(SeekFrom::Start(file.data_offset))?;
        let mut stored = vec![0u8; file.stored_size as usize];
        self.reader
            .read_exact(&mut stored)
            .with_context(|| format!("reading stored bytes of {}", file.name))?;
        Ok(stored)
    }

    /// The member's decoded bytes. Encrypted members are an [`EncryptedMember`]
    /// error, never ciphertext.
    pub fn read(&mut self, file: &IndexFile) -> Result<Vec<u8>> {
        if file.encryption_type.is_encrypted() {
            return Err(EncryptedMember {
                path: file.name.clone(),
                encryption: file.encryption_type.clone(),
            }
            .into());
        }
        let stored = self.read_stored(file)?;
        decode_stored(
            &stored,
            &file.storage_type,
            &file.encryption_type,
            file.uncompressed_size,
            &file.name,
        )
    }

    /// The decoded bytes of the member at `rel`; `Ok(None)` when absent.
    pub fn read_file(&mut self, rel: &str) -> Result<Option<Vec<u8>>> {
        let Some((_, file)) = self.index.file(rel) else {
            return Ok(None);
        };
        let file = file.clone();
        self.read(&file).map(Some)
    }

    /// Checks every member's stored bytes against its CRC and SHA1 block
    /// hashes. Reads the whole data blob once, member by member.
    pub fn verify(&mut self) -> Result<Verification> {
        let members = self
            .index
            .files()
            .map(|(toc, path, file)| (toc.alias.clone(), path, file.clone()))
            .collect::<Vec<_>>();
        let block_size = self.index.header.block_size as usize;
        let mut report = Verification::default();
        for (toc, path, file) in members {
            let stored = self.read_stored(&file)?;
            report.checked_files += 1;
            if file.crc != 0 {
                let mut crc = Crc::new();
                crc.update(&stored);
                if crc.sum() != file.crc {
                    report.crc_mismatches.push(Mismatch {
                        toc: toc.clone(),
                        path: path.clone(),
                        check: "crc32",
                    });
                }
            }
            match self.index.recorded_block_hashes(&file) {
                Some(recorded) => {
                    if block_sha1(&stored, block_size) != recorded {
                        report.sha1_mismatches.push(Mismatch {
                            toc,
                            path,
                            check: "sha1_blocks",
                        });
                    }
                }
                None if file.verification_type == FileVerificationType::SHA1Blocks => {
                    report.unhashed_files += 1;
                }
                None => {}
            }
        }
        Ok(report)
    }
}

fn block_count(stored_size: usize, block_size: usize) -> usize {
    if stored_size == 0 {
        return 1;
    }
    stored_size.div_ceil(block_size.max(1))
}

fn collect_files<'a>(
    folder: &'a IndexFolder,
    prefix: String,
    out: &mut Vec<(String, &'a IndexFile)>,
) {
    for file in &folder.files {
        out.push((child_path(&prefix, &file.name), file));
    }
    for child in &folder.folders {
        collect_files(child, child_path(&prefix, &child.name), out);
    }
}

fn count_files(folder: &IndexFolder) -> usize {
    folder.files.len() + folder.folders.iter().map(count_files).sum::<usize>()
}

fn count_folders(folder: &IndexFolder) -> usize {
    1 + folder.folders.iter().map(count_folders).sum::<usize>()
}

fn child_path(full: &str, name: &str) -> String {
    if full.is_empty() {
        name.to_string()
    } else {
        format!("{full}\\{name}")
    }
}

fn index_folder(
    header: &SgaHeader,
    folders: &[SgaFolderEntry],
    files: &[SgaFileEntry],
    strings: &[u8],
    index: usize,
) -> Result<IndexFolder> {
    let entry = folders
        .get(index)
        .with_context(|| format!("folder index {index} is outside the folder table"))?;
    let full = name_at(strings, entry.name_offset as usize);
    let name = leaf_name(&full);

    let mut file_nodes = Vec::new();
    for i in entry.file_start_index..entry.file_end_index {
        let file = files
            .get(i as usize)
            .with_context(|| format!("file index {i} is outside the file table"))?;
        file_nodes.push(index_file(header, file, strings));
    }

    let mut folder_nodes = Vec::new();
    for i in entry.folder_start_index..entry.folder_end_index {
        folder_nodes.push(index_folder(header, folders, files, strings, i as usize)?);
    }

    Ok(IndexFolder {
        name,
        folders: folder_nodes,
        files: file_nodes,
    })
}

fn index_file(header: &SgaHeader, entry: &SgaFileEntry, strings: &[u8]) -> IndexFile {
    let version = header.version;
    let (storage_type, encryption_type) = if version >= 10 {
        (
            FileStorageType::from_u8(entry.storage_byte & 0x0F),
            FileEncryptionType::from_u8(entry.storage_byte >> 4),
        )
    } else {
        (
            FileStorageType::from_u8(entry.storage_byte),
            FileEncryptionType::None,
        )
    };
    let verification_type = if version >= 7 {
        FileVerificationType::from_u8(entry.verification_byte)
    } else {
        FileVerificationType::None
    };
    IndexFile {
        name: name_at(strings, entry.name_offset as usize),
        data_offset: header.data_offset + entry.data_offset,
        data_order: entry.data_offset,
        stored_size: entry.compressed_length,
        uncompressed_size: entry.uncompressed_size,
        storage_type,
        encryption_type,
        verification_type,
        crc: if version >= 6 { entry.crc } else { 0 },
        hash_offset: if version == 7 {
            entry.hash_offset_v7
        } else {
            entry.hash_offset
        },
    }
}

pub(crate) fn name_at(strings: &[u8], offset: usize) -> String {
    let Some(tail) = strings.get(offset..) else {
        return String::new();
    };
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    String::from_utf8_lossy(&tail[..end]).into_owned()
}

pub(crate) fn leaf_name(full: &str) -> String {
    full.rsplit(['\\', '/']).next().unwrap_or(full).to_string()
}

pub(crate) fn trim_fixed(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}
