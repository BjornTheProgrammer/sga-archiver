use std::collections::HashMap;
use std::io::{BufRead, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use binrw::BinWrite;
use brotli::Decompressor;
use flate2::read::DeflateDecoder;
use flate2::write::ZlibEncoder;
use flate2::{Compression, Crc};
use sha1::{Digest, Sha1};

use crate::entries::{
    FileEncryptionType, FileStorageType, FileVerificationType, HeaderReserved, SgaFileEntry,
    SgaFolderEntry, SgaHeader, SgaToC,
};
use crate::index::{ArchiveIndex, EncryptedMember, IndexFolder};

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
    /// separator, the form the TOC string blob stores. This folder's own name
    /// is not part of them, matching the way a TOC treats its root.
    ///
    /// Depth-first: a folder's own files, then each subfolder in turn.
    ///
    /// The field [`Folder::files`] holds only the *direct* children; this
    /// walks the whole tree.
    pub fn files_recursive(&self) -> Files<'_> {
        Files {
            pending: Vec::new(),
            current: Some((String::new(), self, 0)),
        }
    }

    /// Every folder beneath this folder, paired with its full path.
    ///
    /// Same order and path form as [`Folder::files_recursive`]. This folder is
    /// not yielded; only its descendants, so a leaf yields nothing.
    pub fn folders_recursive(&self) -> Folders<'_> {
        Folders {
            pending: self
                .folders
                .iter()
                .rev()
                .map(|child| (child.name.clone(), child))
                .collect(),
        }
    }
}

/// Iterator over a folder tree's files. See [`Folder::files_recursive`].
pub struct Files<'a> {
    /// Folders not yet reached, each with the prefix its contents take.
    pending: Vec<(String, &'a Folder)>,
    /// The folder being drained, and how far through its files we are.
    current: Option<(String, &'a Folder, usize)>,
}

impl<'a> Iterator for Files<'a> {
    type Item = (String, &'a FileEntry);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (prefix, folder, index) = self.current.as_mut()?;
            if let Some(file) = folder.files.get(*index) {
                *index += 1;
                return Some((child_path(prefix, &file.name), file));
            }
            // This folder is drained; queue its children and move on. Reversed
            // so popping yields them in declaration order.
            let (prefix, folder, _) = self.current.take()?;
            for child in folder.folders.iter().rev() {
                self.pending.push((child_path(&prefix, &child.name), child));
            }
            self.current = self
                .pending
                .pop()
                .map(|(prefix, folder)| (prefix, folder, 0));
        }
    }
}

/// Iterator over a folder tree's folders. See [`Folder::folders_recursive`].
pub struct Folders<'a> {
    pending: Vec<(String, &'a Folder)>,
}

impl<'a> Iterator for Folders<'a> {
    type Item = (String, &'a Folder);

    fn next(&mut self) -> Option<Self::Item> {
        let (path, folder) = self.pending.pop()?;
        for child in folder.folders.iter().rev() {
            self.pending.push((child_path(&path, &child.name), child));
        }
        Some((path, folder))
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
    /// The member's decoded bytes.
    ///
    /// An encrypted member is an [`EncryptedMember`] error: this crate does
    /// not decrypt, and handing back the stored ciphertext as if it were the
    /// member would let a caller write garbage without knowing.
    pub fn decoded(&self) -> Result<Vec<u8>> {
        decode_stored(
            &self.stored_data,
            &self.storage_type,
            &self.encryption_type,
            self.uncompressed_size,
            &self.name,
        )
    }
}

/// Decodes one member's stored bytes according to its storage and
/// encryption types. The single decode path for the eager and lazy readers.
pub(crate) fn decode_stored(
    stored: &[u8],
    storage: &FileStorageType,
    encryption: &FileEncryptionType,
    uncompressed_size: u32,
    name: &str,
) -> Result<Vec<u8>> {
    if encryption.is_encrypted() {
        return Err(EncryptedMember {
            path: name.to_string(),
            encryption: encryption.clone(),
        }
        .into());
    }

    match storage {
        FileStorageType::Store | FileStorageType::Unknown(_) => Ok(stored.to_vec()),
        FileStorageType::StreamCompress | FileStorageType::BufferCompress => {
            let mut cursor = Cursor::new(stored);
            cursor.seek(SeekFrom::Start(2))?;
            let mut decoder = DeflateDecoder::new(cursor);
            let mut out = vec![0u8; uncompressed_size as usize];
            decoder
                .read_exact(&mut out)
                .with_context(|| format!("inflating {name}"))?;
            Ok(out)
        }
        FileStorageType::StreamCompressBrotli | FileStorageType::BufferCompressBrotli => {
            let cursor = Cursor::new(stored);
            let mut decoder = Decompressor::new(cursor, 4096);
            let mut out = vec![0u8; uncompressed_size as usize];
            decoder
                .read_exact(&mut out)
                .with_context(|| format!("brotli-decoding {name}"))?;
            Ok(out)
        }
    }
}

impl Archive {
    pub fn read<R: Read + BufRead + Seek>(reader: &mut R) -> Result<Archive> {
        let index = ArchiveIndex::read(reader)?;
        Archive::from_index(reader, index)
    }

    /// Loads every member's bytes for an already parsed index.
    pub fn from_index<R: Read + Seek>(reader: &mut R, index: ArchiveIndex) -> Result<Archive> {
        let header = index.header;
        let mut tocs = Vec::with_capacity(index.tocs.len());
        for toc in index.tocs {
            tocs.push(Toc {
                alias: toc.alias,
                name: toc.name,
                root: load_folder(reader, toc.root)?,
            });
        }

        let string_blob = {
            reader.seek(SeekFrom::Start(
                header.header_blob_offset + header.string_offset as u64,
            ))?;
            let mut blob = vec![0u8; header.string_length as usize];
            reader.read_exact(&mut blob)?;
            blob
        };
        let layout = [TocLayout::Legacy, TocLayout::Modern]
            .into_iter()
            .find(|&layout| {
                let mut blob = Vec::new();
                build_strings(
                    layout,
                    &tocs,
                    &mut blob,
                    &mut HashMap::new(),
                    &mut HashMap::new(),
                );
                blob == string_blob
            })
            .unwrap_or(TocLayout::Legacy);

        Ok(Archive {
            header_reserved: header.reserved.clone(),
            name: header.name.clone(),
            version: header.version,
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
        build_strings(
            self.layout,
            &self.tocs,
            &mut string_blob,
            &mut folder_str,
            &mut file_str,
        );

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
                let WalkEvent::Visit(full, folder) = event else {
                    continue;
                };
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
                let WalkEvent::Visit(full, folder) = event else {
                    continue;
                };
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
                let WalkEvent::Visit(full, folder) = event else {
                    continue;
                };
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

    /// The entry at `rel`, if the archive holds one.
    ///
    /// `rel` is an archive path separated by `/` or `\\`, matched
    /// exactly, and every TOC is searched. Names compare as stored: this crate
    /// does not fold case, because the format does not and neither do
    /// case-sensitive filesystems. Returns the entry
    /// itself, so callers can inspect size, storage type or CRC without
    /// decoding it — see [`FileEntry::decoded`] for the bytes.
    pub fn file(&self, rel: &str) -> Option<&FileEntry> {
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
                return Some(file);
            }
        }
        None
    }

    /// The decoded bytes of the file at `rel`.
    ///
    /// `Ok(None)` means no such file. A file that is present but cannot be
    /// decoded is an `Err`, which is the distinction [`Archive::read_file`]
    /// cannot make.
    pub fn try_read_file(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        self.file(rel).map(FileEntry::decoded).transpose()
    }

    /// The decoded bytes of the file at `rel`, or `None`.
    ///
    /// A file that is present but fails to decode also reports `None`. Prefer
    /// [`Archive::try_read_file`] where telling those apart matters; this
    /// remains for callers that genuinely only want the bytes or nothing.
    pub fn read_file(&self, rel: &str) -> Option<Vec<u8>> {
        self.try_read_file(rel).ok().flatten()
    }

    /// Every file in the archive, paired with the TOC holding it and its full
    /// path within that TOC.
    ///
    /// Saves callers flat-mapping [`Folder::files_recursive`] over
    /// [`Archive::tocs`] when they need the TOC alongside each file, which the
    /// per-folder walk cannot supply.
    pub fn files(&self) -> impl Iterator<Item = (&Toc, String, &FileEntry)> + '_ {
        self.tocs.iter().flat_map(|toc| {
            toc.root
                .files_recursive()
                .map(move |(path, file)| (toc, path, file))
        })
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
            folder
                .folders
                .retain(|c| !(c.files.is_empty() && c.folders.is_empty()));
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
        let comps: Vec<String> = rel.split(['/', '\\']).map(|s| s.to_lowercase()).collect();
        if let Some((name, dirs)) = comps.split_last() {
            for toc in &mut self.tocs {
                if let Some(folder) = descend_existing(&mut toc.root, dirs) {
                    if let Some(f) = folder
                        .files
                        .iter_mut()
                        .find(|f| f.name.to_lowercase() == *name)
                    {
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
///
/// Names are lowercased on the way in. That is deliberate rather than
/// incidental: every shipped archive is lowercase throughout, and a fresh pack
/// has to match the editor's own build. It does mean a caller's casing is not
/// preserved, so lookups against a pack this crate built should use lowercase.
/// Reads do not fold case — see [`Archive::file`].
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
        let idx = folder.folders.iter().position(|f| f.name == *d)?;
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

pub(crate) fn block_sha1(data: &[u8], block_size: usize) -> Vec<u8> {
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

fn to_fixed(value: &str) -> [u8; 64] {
    let mut out = [0u8; 64];
    let bytes = value.as_bytes();
    let n = bytes.len().min(64);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

fn load_folder<R: Read + Seek>(reader: &mut R, folder: IndexFolder) -> Result<Folder> {
    let mut files = Vec::with_capacity(folder.files.len());
    for file in folder.files {
        reader.seek(SeekFrom::Start(file.data_offset))?;
        let mut stored = vec![0u8; file.stored_size as usize];
        reader
            .read_exact(&mut stored)
            .with_context(|| format!("reading stored bytes of {}", file.name))?;
        files.push(FileEntry {
            name: file.name,
            stored_data: stored,
            uncompressed_size: file.uncompressed_size,
            storage_type: file.storage_type,
            encryption_type: file.encryption_type,
            verification_type: file.verification_type,
            crc: file.crc,
            data_order: Some(file.data_order),
        });
    }
    let mut folders = Vec::with_capacity(folder.folders.len());
    for child in folder.folders {
        folders.push(load_folder(reader, child)?);
    }
    Ok(Folder {
        name: folder.name,
        folders,
        files,
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
