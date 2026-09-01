use std::collections::HashMap;
use std::io::{BufRead, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use binrw::{BinRead, BinWrite};
use brotli::Decompressor;
use flate2::read::DeflateDecoder;
use flate2::write::ZlibEncoder;
use flate2::{Compression, Crc};
use sha1::{Digest, Sha1};

use crate::entires::{
    HeaderReserved,
    FileEncryptionType, FileStorageType, FileVerificationType, SgaFileEntry, SgaFolderEntry,
    SgaHeader, SgaToC,
};

const MAIN_HEADER_SIZE: u64 = 428;
const INDEX_TABLE_SIZE: usize = 44;
const TOC_ENTRY_SIZE: usize = 148;
const FOLDER_ENTRY_SIZE: usize = 20;
const FILE_ENTRY_SIZE: usize = 30;
pub(crate) const DEFAULT_BLOCK_SIZE: u32 = 262144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TocLayout {
    /// Base-game archives and older editor builds: depth-first traversal, no
    /// string deduplication.
    Legacy,
    /// Current editor builds: breadth-first (level-order) traversal with
    /// identical strings deduplicated.
    Modern,
}

#[derive(Debug, Clone)]
pub struct Archive {
    pub header_reserved: HeaderReserved,
    pub name: String,
    pub version: u16,
    pub product: u16,
    pub block_size: u32,
    pub header_encryption_type: FileEncryptionType,
    pub signature: [u8; 256],
    pub layout: TocLayout,
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

impl Folder {
    /// Every file at or beneath this folder, paired with its full path.
    ///
    /// Paths are relative to this folder and joined with the archive's own
    /// separator, the same form the TOC string blob stores. This folder's own
    /// name is not part of them, matching the way a TOC treats its root.
    ///
    /// Order is depth-first: a folder's own files, then each subfolder in
    /// turn.
    pub fn files_recursive(&self) -> Vec<(String, &FileEntry)> {
        fn collect<'a>(prefix: &str, folder: &'a Folder, out: &mut Vec<(String, &'a FileEntry)>) {
            for file in &folder.files {
                out.push((child_path(prefix, &file.name), file));
            }
            for child in &folder.folders {
                collect(&child_path(prefix, &child.name), child, out);
            }
        }
        let mut out = Vec::new();
        collect("", self, &mut out);
        out
    }

    /// The full path of every file at or beneath this folder.
    ///
    /// The paths [`Folder::files_recursive`] produces, without the entries.
    pub fn file_paths(&self) -> Vec<String> {
        self.files_recursive()
            .into_iter()
            .map(|(path, _)| path)
            .collect()
    }

    /// How many files are at or beneath this folder, at any depth.
    pub fn file_count(&self) -> usize {
        self.files.len() + self.folders.iter().map(Folder::file_count).sum::<usize>()
    }

    /// How many folders are beneath this folder, at any depth.
    ///
    /// This folder is not counted; a leaf folder reports zero.
    pub fn folder_count(&self) -> usize {
        self.folders.len()
            + self
                .folders
                .iter()
                .map(Folder::folder_count)
                .sum::<usize>()
    }
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
    /// Original position of this file's data in the archive's data blob, if read
    /// from one. The editor's data-blob order isn't a simple tree traversal, so
    /// preserving it lets a read→write round-trip stay byte-identical.
    pub data_order: Option<u64>,
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

        let layout = [TocLayout::Legacy, TocLayout::Modern]
            .into_iter()
            .find(|&layout| {
                let mut blob = Vec::new();
                build_strings(layout, &tocs, &mut blob, &mut HashMap::new(), &mut HashMap::new());
                blob == string_blob
            })
            .unwrap_or(TocLayout::Legacy);

        Ok(Archive {
            header_reserved: header.reserved.clone(),
            name: header.name.clone(),
            version,
            product: header.product,
            block_size: header.block_size,
            header_encryption_type: header.header_encryption_type.clone(),
            signature: header.signature,
            layout,
            tocs,
        })
    }

    pub fn write<W: Write + Seek>(&self, writer: &mut W) -> Result<()> {
        let mut toc_entries: Vec<SgaToC> = Vec::new();
        let mut folder_entries: Vec<SgaFolderEntry> = Vec::new();
        let mut file_entries: Vec<SgaFileEntry> = Vec::new();
        let mut data_blob: Vec<u8> = Vec::new();
        let mut hash_blob: Vec<u8> = Vec::new();

        let mut string_blob: Vec<u8> = Vec::new();
        let mut folder_str: HashMap<(usize, String), u32> = HashMap::new();
        let mut file_str: HashMap<(usize, String, String), u32> = HashMap::new();
        build_strings(self.layout, &self.tocs, &mut string_blob, &mut folder_str, &mut file_str);

        // Build the data blob. When every file preserves its original position
        // (a read→write round-trip), lay the files out in that order so the blob
        let mut file_data_off: HashMap<(usize, String, String), u64> = HashMap::new();
        let mut all_files: Vec<(usize, String, String, &FileEntry)> = Vec::new();
        for (ti, toc) in self.tocs.iter().enumerate() {
            for event in walk(self.layout, &toc.root) {
                if let WalkEvent::Visit(full, folder) = event {
                    for file in &folder.files {
                        all_files.push((ti, full.clone(), file.name.clone(), file));
                    }
                }
            }
        }
        if all_files.iter().all(|(_, _, _, f)| f.data_order.is_some()) {
            all_files.sort_by_key(|(_, _, _, f)| f.data_order.unwrap());
        }
        for (ti, full, name, file) in &all_files {
            file_data_off.insert((*ti, full.clone(), name.clone()), data_blob.len() as u64);
            data_blob.extend_from_slice(&file.stored_data);
        }

        let block_size = if self.block_size == 0 {
            DEFAULT_BLOCK_SIZE
        } else {
            self.block_size
        } as usize;

        let mut folder_file_range: HashMap<(usize, String), (u32, u32)> = HashMap::new();
        let mut toc_file_ranges: Vec<(u32, u32)> = Vec::new();
        for (ti, toc) in self.tocs.iter().enumerate() {
            let start = file_entries.len() as u32;
            for event in walk(self.layout, &toc.root) {
                let WalkEvent::Visit(full, folder) = event else { continue };
                let folder_start = file_entries.len() as u32;
                for file in &folder.files {
                    let key = (ti, full.clone(), file.name.clone());
                    push_file_entry(
                        file,
                        &key,
                        &mut file_entries,
                        &file_str,
                        &file_data_off,
                        &mut hash_blob,
                        block_size,
                    );
                }
                folder_file_range.insert((ti, full), (folder_start, file_entries.len() as u32));
            }
            toc_file_ranges.push((start, file_entries.len() as u32));
        }

        let mut folder_range: HashMap<(usize, String), (u32, u32)> = HashMap::new();
        let mut counter = 0u32;
        for (ti, toc) in self.tocs.iter().enumerate() {
            counter += 1;
            for event in walk(self.layout, &toc.root) {
                let WalkEvent::Visit(full, folder) = event else { continue };
                let start = counter;
                counter += folder.folders.len() as u32;
                folder_range.insert((ti, full), (start, counter));
            }
        }

        // Folder table: emitted so each folder's children occupy the contiguous
        for (ti, toc) in self.tocs.iter().enumerate() {
            let toc_folder_start = folder_entries.len() as u32;
            let root_index = folder_entries.len() as u32;

            let mk = |full: &str| {
                let (folder_start, folder_end) = folder_range[&(ti, full.to_string())];
                let (file_start, file_end) = folder_file_range[&(ti, full.to_string())];
                SgaFolderEntry {
                    name_offset: folder_str[&(ti, full.to_string())],
                    folder_start_index: folder_start,
                    folder_end_index: folder_end,
                    file_start_index: file_start,
                    file_end_index: file_end,
                }
            };

            folder_entries.push(mk(""));
            for event in walk(self.layout, &toc.root) {
                let WalkEvent::Visit(full, folder) = event else { continue };
                for child in &folder.folders {
                    folder_entries.push(mk(&child_path(&full, &child.name)));
                }
            }

            let (toc_file_start, toc_file_end) = toc_file_ranges[ti];
            toc_entries.push(SgaToC {
                alias: to_fixed(&toc.alias),
                name: to_fixed(&toc.name),
                folder_start_index: toc_folder_start,
                folder_end_index: folder_entries.len() as u32,
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
            version: self.version,
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
            reserved: self.header_reserved.clone(),
        };

        header.write_main_header(writer)?;
        writer.write_all(&data_blob)?;
        header.write_index_table(writer)?;
        for entry in &toc_entries {
            entry.write_le_args(writer, (self.version,))?;
        }
        for entry in &folder_entries {
            entry.write_le_args(writer, (self.version,))?;
        }
        for entry in &file_entries {
            entry.write_le_args(writer, (self.version,))?;
        }
        writer.write_all(&string_blob)?;
        writer.write_all(&hash_blob)?;

        Ok(())
    }

    pub fn extract_to<P: AsRef<Path>>(&self, out: P) -> Result<Vec<PathBuf>> {
        let mut written = Vec::new();
        for toc in &self.tocs {
            extract_folder(&toc.root, out.as_ref(), &mut written)?;
        }
        Ok(written)
    }

    /// Returns the decoded bytes of the file at `rel` (a `/`-separated archive
    /// path, case-insensitive), searching every TOC. Used to read a packed
    /// `.rgm`/`.layer` back out so it can be patched and re-inserted.
    pub fn read_file(&self, rel: &str) -> Option<Vec<u8>> {
        let comps: Vec<String> = rel.split(['/', '\\']).map(|s| s.to_lowercase()).collect();
        let (name, dirs) = comps.split_last()?;
        for toc in &self.tocs {
            let mut folder = &toc.root;
            let mut ok = true;
            for d in dirs {
                match folder.folders.iter().find(|f| &f.name == d) {
                    Some(f) => folder = f,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                if let Some(f) = folder.files.iter().find(|f| &f.name == name) {
                    return f.decoded().ok();
                }
            }
        }
        None
    }

    /// Removes every file whose lowercased name matches `pred`, across all TOCs,
    /// returning how many were removed. Empty folders are left in place (the
    /// game tolerates them). Used to strip files the game forbids in a mod pack,
    /// e.g. streaming `*_packed.rrtex`.
    pub fn remove_files_where(&mut self, pred: impl Fn(&str) -> bool) -> usize {
        fn walk(folder: &mut Folder, pred: &impl Fn(&str) -> bool, n: &mut usize) {
            let before = folder.files.len();
            folder.files.retain(|f| !pred(&f.name.to_lowercase()));
            *n += before - folder.files.len();
            for child in &mut folder.folders {
                walk(child, pred, n);
            }
        }
        let mut n = 0;
        for toc in &mut self.tocs {
            walk(&mut toc.root, &pred, &mut n);
        }
        n
    }

    /// Inserts (or replaces) a stored file at `rel` inside the TOC named
    /// `toc_alias`, creating that TOC if it doesn't exist. Mods route files by
    /// purpose into separate TOCs — `info` (mod descriptor), `locale`
    /// (localization), `data` (everything else) — and the game rejects a file
    /// that lands in the wrong one.
    pub fn upsert_stored_in(&mut self, toc_alias: &str, rel: &str, data: Vec<u8>) {
        let relp = PathBuf::from(rel.replace('\\', "/"));
        let toc = get_or_create_toc(&mut self.tocs, toc_alias);
        insert_or_replace(&mut toc.root, &relp, stored_file(&relp, data));
    }

    /// Re-stores art render-resources to match how the base game packages them:
    /// `.rrtex` as `Store`, other art as `BufferCompress`, all with `verify =
    /// None` (the base game hashes nothing here). The a4etk burn used
    /// `SHA1Blocks`/`CRC`, which the game may refuse for mod art. Returns count.
    pub fn repackage_art(&mut self) -> Result<usize> {
        fn kind(name: &str) -> Option<FileStorageType> {
            if name.ends_with(".rrtex") {
                Some(FileStorageType::Store)
            } else if name.ends_with(".rrmaterial")
                || name.ends_with(".rrgeom")
                || name.ends_with(".rgm")
                || name.ends_with(".rgo")
            {
                Some(FileStorageType::BufferCompress)
            } else {
                None
            }
        }
        fn walk(folder: &mut Folder, n: &mut usize) -> Result<()> {
            for f in &mut folder.files {
                if let Some(storage) = kind(&f.name.to_lowercase()) {
                    let decoded = f.decoded()?;
                    let stored = encode(&decoded, &storage)?;
                    let mut crc = Crc::new();
                    crc.update(&stored);
                    f.crc = crc.sum();
                    f.uncompressed_size = decoded.len() as u32;
                    f.stored_data = stored;
                    f.storage_type = storage;
                    f.verification_type = FileVerificationType::None;
                    *n += 1;
                }
            }
            for child in &mut folder.folders {
                walk(child, n)?;
            }
            Ok(())
        }
        let mut n = 0;
        for toc in &mut self.tocs {
            walk(&mut toc.root, &mut n)?;
        }
        Ok(n)
    }

    /// Removes folders that (recursively) contain no files, across all TOCs.
    /// Editor archives never carry empty folders; leaving them after a delete
    /// can make the game reject the mod's file structure.
    pub fn prune_empty_folders(&mut self) {
        fn prune(folder: &mut Folder) {
            for child in &mut folder.folders {
                prune(child);
            }
            folder.folders.retain(|c| !(c.files.is_empty() && c.folders.is_empty()));
        }
        for toc in &mut self.tocs {
            prune(&mut toc.root);
        }
    }

    /// Inserts (or replaces) a stored, uncompressed file at `rel`. If a file
    /// already exists at that path in any TOC it is replaced in place; otherwise
    /// it is added under the first TOC. Fresh entries have no `data_order`, so
    /// they append after the preserved originals when written.
    pub fn upsert_stored(&mut self, rel: &str, data: Vec<u8>) {
        if self.tocs.is_empty() {
            let _ = get_or_create_toc(&mut self.tocs, "data");
        }
        let relp = PathBuf::from(rel.replace('\\', "/"));
        let comps: Vec<String> =
            rel.split(['/', '\\']).map(|s| s.to_lowercase()).collect();
        if let Some((name, dirs)) = comps.split_last() {
            for toc in &mut self.tocs {
                if let Some(folder) = descend_existing(&mut toc.root, dirs) {
                    if let Some(f) = folder.files.iter_mut().find(|f| &f.name == name) {
                        *f = stored_file(&relp, data);
                        return;
                    }
                }
            }
        }
        insert_or_replace(&mut self.tocs[0].root, &relp, stored_file(&relp, data));
    }

}

pub(crate) fn stored_file(rel: &Path, data: Vec<u8>) -> FileEntry {
    let name = rel
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut crc = Crc::new();
    crc.update(&data);
    FileEntry {
        name,
        uncompressed_size: data.len() as u32,
        stored_data: data,
        storage_type: FileStorageType::Store,
        encryption_type: FileEncryptionType::None,
        verification_type: FileVerificationType::SHA1Blocks,
        crc: crc.sum(),
        data_order: None,
    }
}

pub(crate) fn get_or_create_toc<'a>(tocs: &'a mut Vec<Toc>, alias: &str) -> &'a mut Toc {
    if let Some(idx) = tocs.iter().position(|t| t.alias == alias) {
        return &mut tocs[idx];
    }
    tocs.push(Toc {
        alias: alias.to_string(),
        name: alias.to_string(),
        root: Folder {
            name: String::new(),
            folders: Vec::new(),
            files: Vec::new(),
        },
    });
    tocs.last_mut().unwrap()
}

/// Inserts `file` at `rel` under `root`, replacing any file already there.
pub(crate) fn insert_or_replace(root: &mut Folder, rel: &Path, mut file: FileEntry) {
    file.name = file.name.to_lowercase();
    let folder = descend(root, rel);
    match folder.files.iter().position(|f| f.name == file.name) {
        Some(i) => folder.files[i] = file,
        None => folder.files.push(file),
    }
}

/// Matches a `.burnproj` include glob (with `\` separators, `**` across
/// directories and `*`/`?` within a segment) against a `/`-separated path.

pub(crate) fn insert_file(root: &mut Folder, rel: &Path, mut file: FileEntry) {
    // AoE4 archives use all-lowercase paths; the engine lowercases lookups, so
    // a mixed-case entry (e.g. a win condition's `.scar`) would never be found.
    file.name = file.name.to_lowercase();
    let folder = descend(root, rel);
    folder.files.push(file);
}

/// Walks/creates the (lower-cased) folder chain for `rel`'s parent, returning
/// the folder its file belongs in.
/// Navigates to the existing folder named by `dirs` (lowercased directory
/// components), or `None` if any component is missing — no folders are created.
fn descend_existing<'a>(root: &'a mut Folder, dirs: &[String]) -> Option<&'a mut Folder> {
    let mut folder = root;
    for d in dirs {
        let idx = folder.folders.iter().position(|f| &f.name == d)?;
        folder = &mut folder.folders[idx];
    }
    Some(folder)
}

fn descend<'a>(root: &'a mut Folder, rel: &Path) -> &'a mut Folder {
    let mut folder = root;
    if let Some(parent) = rel.parent() {
        for comp in parent.components() {
            if let std::path::Component::Normal(os) = comp {
                let name = os.to_string_lossy().to_lowercase();
                let idx = match folder.folders.iter().position(|f| f.name == name) {
                    Some(i) => i,
                    None => {
                        folder.folders.push(Folder {
                            name,
                            folders: Vec::new(),
                            files: Vec::new(),
                        });
                        folder.folders.len() - 1
                    }
                };
                folder = &mut folder.folders[idx];
            }
        }
    }
    folder
}

pub(crate) fn encode(data: &[u8], storage: &FileStorageType) -> Result<Vec<u8>> {
    match storage {
        FileStorageType::Store | FileStorageType::Unknown(_) => Ok(data.to_vec()),
        FileStorageType::StreamCompress | FileStorageType::BufferCompress => {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(data)?;
            Ok(encoder.finish()?)
        }
        FileStorageType::StreamCompressBrotli | FileStorageType::BufferCompressBrotli => {
            Err(anyhow!("Brotli re-compression is not supported"))
        }
    }
}

enum WalkEvent<'a> {
    Discover(String),
    Visit(String, &'a Folder),
}

/// The single place the two layout generations' traversals exist. Legacy
/// walks depth-first and discovers each folder immediately before its own
/// visit; Modern walks breadth-first and discovers children during their
fn walk<'a>(layout: TocLayout, root: &'a Folder) -> Vec<WalkEvent<'a>> {
    let mut events = Vec::new();
    match layout {
        TocLayout::Legacy => {
            fn dfs<'a>(full: String, folder: &'a Folder, events: &mut Vec<WalkEvent<'a>>) {
                events.push(WalkEvent::Discover(full.clone()));
                events.push(WalkEvent::Visit(full.clone(), folder));
                for child in &folder.folders {
                    dfs(child_path(&full, &child.name), child, events);
                }
            }
            dfs(String::new(), root, &mut events);
        }
        TocLayout::Modern => {
            events.push(WalkEvent::Discover(String::new()));
            let mut queue = std::collections::VecDeque::from([(String::new(), root)]);
            while let Some((full, folder)) = queue.pop_front() {
                events.push(WalkEvent::Visit(full.clone(), folder));
                for child in &folder.folders {
                    let full = child_path(&full, &child.name);
                    events.push(WalkEvent::Discover(full.clone()));
                    queue.push_back((full, child));
                }
            }
        }
    }
    events
}

fn child_path(full: &str, name: &str) -> String {
    if full.is_empty() {
        name.to_string()
    } else {
        format!("{full}\\{name}")
    }
}

fn build_strings(
    layout: TocLayout,
    tocs: &[Toc],
    blob: &mut Vec<u8>,
    folder_str: &mut HashMap<(usize, String), u32>,
    file_str: &mut HashMap<(usize, String, String), u32>,
) {
    let mut pool: HashMap<String, u32> = HashMap::new();
    let mut add = |blob: &mut Vec<u8>, value: &str| -> u32 {
        if layout == TocLayout::Modern {
            if let Some(&offset) = pool.get(value) {
                return offset;
            }
            let offset = append_str(blob, value);
            pool.insert(value.to_string(), offset);
            offset
        } else {
            append_str(blob, value)
        }
    };
    for (ti, toc) in tocs.iter().enumerate() {
        for event in walk(layout, &toc.root) {
            match event {
                WalkEvent::Discover(path) => {
                    let offset = add(blob, &path);
                    folder_str.insert((ti, path), offset);
                }
                WalkEvent::Visit(path, folder) => {
                    for file in &folder.files {
                        let offset = add(blob, &file.name);
                        file_str.insert((ti, path.clone(), file.name.clone()), offset);
                    }
                }
            }
        }
    }
}

fn push_file_entry(
    file: &FileEntry,
    key: &(usize, String, String),
    file_entries: &mut Vec<SgaFileEntry>,
    file_str: &HashMap<(usize, String, String), u32>,
    file_data_off: &HashMap<(usize, String, String), u64>,
    hash_blob: &mut Vec<u8>,
    block_size: usize,
) {
    let hash_off = if file.verification_type == FileVerificationType::SHA1Blocks {
        let offset = hash_blob.len() as u32;
        hash_blob.extend_from_slice(&block_sha1(&file.stored_data, block_size));
        offset
    } else {
        hash_blob.len() as u32
    };
    file_entries.push(SgaFileEntry {
        name_offset: file_str[key],
        hash_offset: hash_off,
        data_offset: file_data_off[key],
        compressed_length: file.stored_data.len() as u32,
        uncompressed_size: file.uncompressed_size,
        unknown: 0,
        verification_byte: file.verification_type.to_u8(),
        storage_byte: (file.encryption_type.to_u8() << 4) | file.storage_type.to_u8(),
        crc: file.crc,
        hash_offset_v7: 0,
    });
}
/// Appends a NUL-terminated string to the blob, returning its offset.
fn append_str(blob: &mut Vec<u8>, value: &str) -> u32 {
    let offset = blob.len() as u32;
    blob.extend_from_slice(value.as_bytes());
    blob.push(0);
    offset
}

/// Depth-first pass building the string blob: each folder emits its full path,

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
        data_order: Some(entry.data_offset),
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

