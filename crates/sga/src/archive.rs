use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use binrw::{BinRead, BinWrite};
use brotli::Decompressor;
use flate2::read::DeflateDecoder;
use sha1::{Digest, Sha1};

use crate::entires::{
    FileEncryptionType, FileStorageType, FileVerificationType, SgaFileEntry, SgaFolderEntry,
    SgaHeader, SgaToC,
};

const MAIN_HEADER_SIZE: u64 = 428;
const INDEX_TABLE_SIZE: usize = 44;
const TOC_ENTRY_SIZE: usize = 148;
const FOLDER_ENTRY_SIZE: usize = 20;
const FILE_ENTRY_SIZE: usize = 30;
const DEFAULT_BLOCK_SIZE: u32 = 262144;

#[derive(Debug, Clone)]
pub struct Archive {
    pub name: String,
    pub version: u16,
    pub product: u16,
    pub block_size: u32,
    pub header_encryption_type: FileEncryptionType,
    pub signature: [u8; 256],
    pub tocs: Vec<Toc>,
}

#[derive(Debug, Clone)]
pub struct Toc {
    pub alias: String,
    pub name: String,
    pub root: Folder,
}

#[derive(Debug, Clone)]
pub struct Folder {
    pub name: String,
    pub folders: Vec<Folder>,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub stored_data: Vec<u8>,
    pub uncompressed_size: u32,
    pub storage_type: FileStorageType,
    pub encryption_type: FileEncryptionType,
    pub verification_type: FileVerificationType,
    pub crc: u32,
}

impl FileEntry {
    pub fn decoded(&self) -> Result<Vec<u8>> {
        if self.encryption_type.is_encrypted() {
            return Ok(self.stored_data.clone());
        }

        match &self.storage_type {
            FileStorageType::Store | FileStorageType::Unknown(_) => Ok(self.stored_data.clone()),
            FileStorageType::StreamCompress | FileStorageType::BufferCompress => {
                let mut cursor = Cursor::new(&self.stored_data);
                cursor.seek(SeekFrom::Start(2))?;
                let mut decoder = DeflateDecoder::new(cursor);
                let mut out = vec![0u8; self.uncompressed_size as usize];
                decoder.read_exact(&mut out)?;
                Ok(out)
            }
            FileStorageType::StreamCompressBrotli | FileStorageType::BufferCompressBrotli => {
                let cursor = Cursor::new(&self.stored_data);
                let mut decoder = Decompressor::new(cursor, 4096);
                let mut out = vec![0u8; self.uncompressed_size as usize];
                decoder.read_exact(&mut out)?;
                Ok(out)
            }
        }
    }
}

impl Archive {
    pub fn read<R: Read + BufRead + Seek>(reader: &mut R) -> Result<Archive> {
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

        let mut tocs = Vec::with_capacity(toc_entries.len());
        for te in &toc_entries {
            let root = build_folder(
                reader,
                &header,
                &folder_entries,
                &file_entries,
                &string_blob,
                te.folder_root_index as usize,
                version,
            )?;
            tocs.push(Toc {
                alias: trim_fixed(&te.alias),
                name: trim_fixed(&te.name),
                root,
            });
        }

        Ok(Archive {
            name: header.name.clone(),
            version,
            product: header.product,
            block_size: header.block_size,
            header_encryption_type: header.header_encryption_type.clone(),
            signature: header.signature,
            tocs,
        })
    }

    pub fn write<W: Write + Seek>(&self, writer: &mut W) -> Result<()> {
        let mut toc_entries: Vec<SgaToC> = Vec::new();
        let mut folder_entries: Vec<SgaFolderEntry> = Vec::new();
        let mut file_entries: Vec<SgaFileEntry> = Vec::new();
        let mut string_blob: Vec<u8> = Vec::new();
        let mut string_offsets: HashMap<String, u32> = HashMap::new();
        let mut data_blob: Vec<u8> = Vec::new();
        let mut hash_blob: Vec<u8> = Vec::new();
        let mut hash_offsets: HashMap<Vec<u8>, u32> = HashMap::new();

        let block_size = if self.block_size == 0 {
            DEFAULT_BLOCK_SIZE
        } else {
            self.block_size
        } as usize;

        for toc in &self.tocs {
            let toc_folder_start = folder_entries.len() as u32;
            let toc_file_start = file_entries.len() as u32;
            let root_index = folder_entries.len() as u32;

            let root_name_off = intern("", &mut string_blob, &mut string_offsets);
            let root_entry_index = folder_entries.len();
            folder_entries.push(SgaFolderEntry {
                name_offset: root_name_off,
                folder_start_index: 0,
                folder_end_index: 0,
                file_start_index: 0,
                file_end_index: 0,
            });

            let mut queue: VecDeque<(usize, &Folder, String)> = VecDeque::new();
            queue.push_back((root_entry_index, &toc.root, String::new()));

            while let Some((idx, folder, full)) = queue.pop_front() {
                let file_start = file_entries.len() as u32;
                for file in &folder.files {
                    let name_off = intern(&file.name, &mut string_blob, &mut string_offsets);
                    let data_off = data_blob.len() as u64;
                    data_blob.extend_from_slice(&file.stored_data);
                    let storage_byte =
                        (file.encryption_type.to_u8() << 4) | file.storage_type.to_u8();
                    let hash_off = if file.verification_type == FileVerificationType::SHA1Blocks {
                        let hash = block_sha1(&file.stored_data, block_size);
                        intern_hash(hash, &mut hash_blob, &mut hash_offsets)
                    } else {
                        0
                    };
                    file_entries.push(SgaFileEntry {
                        name_offset: name_off,
                        hash_offset: hash_off,
                        data_offset: data_off,
                        compressed_length: file.stored_data.len() as u32,
                        uncompressed_size: file.uncompressed_size,
                        unknown: 0,
                        verification_byte: file.verification_type.to_u8(),
                        storage_byte,
                        crc: file.crc,
                        hash_offset_v7: 0,
                    });
                }
                let file_end = file_entries.len() as u32;

                let folder_start = folder_entries.len() as u32;
                let mut children: Vec<(usize, &Folder, String)> = Vec::new();
                for child in &folder.folders {
                    let child_full = if full.is_empty() {
                        child.name.clone()
                    } else {
                        format!("{}\\{}", full, child.name)
                    };
                    let name_off = intern(&child_full, &mut string_blob, &mut string_offsets);
                    let cidx = folder_entries.len();
                    folder_entries.push(SgaFolderEntry {
                        name_offset: name_off,
                        folder_start_index: 0,
                        folder_end_index: 0,
                        file_start_index: 0,
                        file_end_index: 0,
                    });
                    children.push((cidx, child, child_full));
                }
                let folder_end = folder_entries.len() as u32;

                folder_entries[idx].folder_start_index = folder_start;
                folder_entries[idx].folder_end_index = folder_end;
                folder_entries[idx].file_start_index = file_start;
                folder_entries[idx].file_end_index = file_end;

                for child in children {
                    queue.push_back(child);
                }
            }

            let toc_folder_end = folder_entries.len() as u32;
            let toc_file_end = file_entries.len() as u32;

            toc_entries.push(SgaToC {
                alias: to_fixed(&toc.alias),
                name: to_fixed(&toc.name),
                folder_start_index: toc_folder_start,
                folder_end_index: toc_folder_end,
                file_start_index: toc_file_start,
                file_end_index: toc_file_end,
                folder_root_index: root_index,
            });
        }

        let toc_data_offset = INDEX_TABLE_SIZE as u32;
        let folder_data_offset = toc_data_offset + (toc_entries.len() * TOC_ENTRY_SIZE) as u32;
        let file_data_offset =
            folder_data_offset + (folder_entries.len() * FOLDER_ENTRY_SIZE) as u32;
        let string_offset = file_data_offset + (file_entries.len() * FILE_ENTRY_SIZE) as u32;
        let string_length = string_blob.len() as u32;
        let file_hash_offset = string_offset + string_length;
        let file_hash_length = hash_blob.len() as u32;
        let header_blob_length = file_hash_offset + file_hash_length;

        let data_offset = MAIN_HEADER_SIZE;
        let data_blob_length = data_blob.len() as u64;
        let header_blob_offset = data_offset + data_blob_length;

        let header = SgaHeader {
            magic: *b"_ARCHIVE",
            version: 11,
            product: self.product,
            name: self.name.clone(),
            header_blob_offset,
            header_blob_length,
            data_offset,
            data_blob_length,
            toc_data_offset,
            toc_data_count: toc_entries.len() as u32,
            folder_data_offset,
            folder_data_count: folder_entries.len() as u32,
            file_data_offset,
            file_data_count: file_entries.len() as u32,
            string_offset,
            string_length,
            block_size: if self.block_size == 0 {
                DEFAULT_BLOCK_SIZE
            } else {
                self.block_size
            },
            header_encryption_type: self.header_encryption_type.clone(),
            signature: self.signature,
            file_hash_offset,
            file_hash_length,
        };

        header.write_main_header(writer)?;
        writer.write_all(&data_blob)?;
        header.write_index_table(writer)?;
        for entry in &toc_entries {
            entry.write_le_args(writer, (11u16,))?;
        }
        for entry in &folder_entries {
            entry.write_le_args(writer, (11u16,))?;
        }
        for entry in &file_entries {
            entry.write_le_args(writer, (11u16,))?;
        }
        writer.write_all(&string_blob)?;
        writer.write_all(&hash_blob)?;

        Ok(())
    }

    pub fn from_dir<P: AsRef<Path>>(name: &str, dir: P) -> Result<Archive> {
        let root = build_from_dir(dir.as_ref())?;
        Ok(Archive {
            name: name.to_string(),
            version: 11,
            product: 0,
            block_size: DEFAULT_BLOCK_SIZE,
            header_encryption_type: FileEncryptionType::None,
            signature: [0u8; 256],
            tocs: vec![Toc {
                alias: "data".to_string(),
                name: "data".to_string(),
                root,
            }],
        })
    }

    pub fn extract_to<P: AsRef<Path>>(&self, out: P) -> Result<Vec<PathBuf>> {
        let mut written = Vec::new();
        for toc in &self.tocs {
            extract_folder(&toc.root, out.as_ref(), &mut written)?;
        }
        Ok(written)
    }
}

fn intern(value: &str, blob: &mut Vec<u8>, map: &mut HashMap<String, u32>) -> u32 {
    if let Some(&offset) = map.get(value) {
        return offset;
    }
    let offset = blob.len() as u32;
    blob.extend_from_slice(value.as_bytes());
    blob.push(0);
    map.insert(value.to_string(), offset);
    offset
}

fn block_sha1(data: &[u8], block_size: usize) -> Vec<u8> {
    let block_size = block_size.max(1);
    let mut out = Vec::new();
    if data.is_empty() {
        out.extend_from_slice(&Sha1::digest(data));
        return out;
    }
    for chunk in data.chunks(block_size) {
        out.extend_from_slice(&Sha1::digest(chunk));
    }
    out
}

fn intern_hash(hash: Vec<u8>, blob: &mut Vec<u8>, map: &mut HashMap<Vec<u8>, u32>) -> u32 {
    if let Some(&offset) = map.get(&hash) {
        return offset;
    }
    let offset = blob.len() as u32;
    blob.extend_from_slice(&hash);
    map.insert(hash, offset);
    offset
}

fn name_at(strings: &[u8], offset: usize) -> String {
    let end = strings[offset..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| offset + p)
        .unwrap_or(strings.len());
    String::from_utf8_lossy(&strings[offset..end]).into_owned()
}

fn leaf_name(full: &str) -> String {
    full.rsplit(|c| c == '\\' || c == '/')
        .next()
        .unwrap_or("")
        .to_string()
}

fn trim_fixed(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn to_fixed(value: &str) -> [u8; 64] {
    let mut out = [0u8; 64];
    let bytes = value.as_bytes();
    let n = bytes.len().min(64);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

fn build_folder<R: Read + Seek>(
    reader: &mut R,
    header: &SgaHeader,
    folders: &[SgaFolderEntry],
    files: &[SgaFileEntry],
    strings: &[u8],
    index: usize,
    version: u16,
) -> Result<Folder> {
    let entry = &folders[index];
    let full = name_at(strings, entry.name_offset as usize);
    let name = leaf_name(&full);

    let mut file_nodes = Vec::new();
    for i in entry.file_start_index..entry.file_end_index {
        file_nodes.push(build_file(reader, header, &files[i as usize], strings, version)?);
    }

    let mut folder_nodes = Vec::new();
    for i in entry.folder_start_index..entry.folder_end_index {
        folder_nodes.push(build_folder(
            reader,
            header,
            folders,
            files,
            strings,
            i as usize,
            version,
        )?);
    }

    Ok(Folder {
        name,
        folders: folder_nodes,
        files: file_nodes,
    })
}

fn build_file<R: Read + Seek>(
    reader: &mut R,
    header: &SgaHeader,
    entry: &SgaFileEntry,
    strings: &[u8],
    version: u16,
) -> Result<FileEntry> {
    let name = name_at(strings, entry.name_offset as usize);

    reader.seek(SeekFrom::Start(header.data_offset + entry.data_offset))?;
    let mut stored = vec![0u8; entry.compressed_length as usize];
    reader.read_exact(&mut stored)?;

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
            .unwrap_or(FileVerificationType::None)
    } else {
        FileVerificationType::None
    };

    Ok(FileEntry {
        name,
        stored_data: stored,
        uncompressed_size: entry.uncompressed_size,
        storage_type,
        encryption_type,
        verification_type,
        crc: if version >= 6 { entry.crc } else { 0 },
    })
}

fn extract_folder(folder: &Folder, base: &Path, written: &mut Vec<PathBuf>) -> Result<()> {
    let dir = base.join(&folder.name);
    std::fs::create_dir_all(&dir)?;

    for file in &folder.files {
        let data = file.decoded()?;
        let path = dir.join(&file.name);
        std::fs::write(&path, &data)?;
        written.push(path);
    }

    for sub in &folder.folders {
        extract_folder(sub, &dir, written)?;
    }

    Ok(())
}

fn build_from_dir(dir: &Path) -> Result<Folder> {
    let mut folders = Vec::new();
    let mut files = Vec::new();

    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            let mut sub = build_from_dir(&path)?;
            sub.name = file_name;
            folders.push(sub);
        } else {
            let data = std::fs::read(&path)?;
            let len = data.len() as u32;
            files.push(FileEntry {
                name: file_name,
                stored_data: data,
                uncompressed_size: len,
                storage_type: FileStorageType::Store,
                encryption_type: FileEncryptionType::None,
                verification_type: FileVerificationType::SHA1Blocks,
                crc: 0,
            });
        }
    }

    Ok(Folder {
        name: String::new(),
        folders,
        files,
    })
}
