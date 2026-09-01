//! Walking a folder tree: full paths, and counts.

use sga::entires::{FileEncryptionType, FileStorageType, FileVerificationType};
use sga::{FileEntry, Folder};

fn stored(name: &str) -> FileEntry {
    FileEntry {
        name: name.to_string(),
        stored_data: Vec::new(),
        uncompressed_size: 0,
        storage_type: FileStorageType::Store,
        encryption_type: FileEncryptionType::None,
        verification_type: FileVerificationType::None,
        crc: 0,
        data_order: None,
    }
}

fn folder(name: &str, files: &[&str], folders: Vec<Folder>) -> Folder {
    Folder {
        name: name.to_string(),
        files: files.iter().map(|name| stored(name)).collect(),
        folders,
    }
}

/// A root with files at three depths, and an empty folder for good measure.
fn sample() -> Folder {
    folder(
        "",
        &["top.txt"],
        vec![
            folder(
                "art",
                &["mod.rrtex"],
                vec![
                    folder("scenario", &["house.rgm", "house.rgo"], vec![]),
                    folder("empty", &[], vec![]),
                ],
            ),
            folder("attrib", &["house.rgd"], vec![]),
        ],
    )
}

#[test]
fn paths_are_relative_to_the_folder_and_use_the_archive_separator() {
    // The root's own name is not part of the paths, matching the way a TOC
    // treats its root, and the separator is the one the string blob stores.
    assert_eq!(
        sample().file_paths(),
        vec![
            "top.txt",
            r"art\mod.rrtex",
            r"art\scenario\house.rgm",
            r"art\scenario\house.rgo",
            r"attrib\house.rgd",
        ]
    );
}

#[test]
fn a_nested_folder_is_walked_as_its_own_root() {
    let root = sample();
    let art = &root.folders[0];
    assert_eq!(
        art.file_paths(),
        vec!["mod.rrtex", r"scenario\house.rgm", r"scenario\house.rgo"]
    );
}

#[test]
fn entries_come_back_alongside_their_paths() {
    let root = sample();
    let files = root.files_recursive();
    assert_eq!(files.len(), root.file_paths().len());
    let (path, entry) = &files[2];
    assert_eq!(path, r"art\scenario\house.rgm");
    assert_eq!(entry.name, "house.rgm");
}

#[test]
fn counts_cover_every_depth_and_exclude_the_folder_itself() {
    let root = sample();
    assert_eq!(root.file_count(), 5);
    // art, art\scenario, art\empty, attrib.
    assert_eq!(root.folder_count(), 4);

    let leaf = folder("leaf", &["one.txt"], vec![]);
    assert_eq!(leaf.file_count(), 1);
    assert_eq!(leaf.folder_count(), 0);
}

#[test]
fn an_empty_tree_walks_to_nothing() {
    let empty = folder("", &[], vec![]);
    assert!(empty.file_paths().is_empty());
    assert!(empty.files_recursive().is_empty());
    assert_eq!(empty.file_count(), 0);
    assert_eq!(empty.folder_count(), 0);
}
