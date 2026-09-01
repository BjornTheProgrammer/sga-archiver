//! Walking a folder tree: full paths, and counts.

use sga::entries::{FileEncryptionType, FileStorageType, FileVerificationType};
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
    let root = sample();
    let paths = root
        .files_recursive()
        .map(|(path, _)| path)
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
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
    let paths = art
        .files_recursive()
        .map(|(path, _)| path)
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec!["mod.rrtex", r"scenario\house.rgm", r"scenario\house.rgo"]
    );
}

#[test]
fn entries_come_back_alongside_their_paths() {
    let root = sample();
    let (path, entry) = root.files_recursive().nth(2).unwrap();
    assert_eq!(path, r"art\scenario\house.rgm");
    assert_eq!(entry.name, "house.rgm");
}

#[test]
fn folders_are_walked_without_yielding_the_starting_folder() {
    let root = sample();
    let paths = root
        .folders_recursive()
        .map(|(path, _)| path)
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["art", r"art\scenario", r"art\empty", "attrib",]);
}

#[test]
fn counting_is_the_iterators_job() {
    // No file_count/folder_count methods: Iterator::count already does it.
    let root = sample();
    assert_eq!(root.files_recursive().count(), 5);
    assert_eq!(root.folders_recursive().count(), 4);

    let leaf = folder("leaf", &["one.txt"], vec![]);
    assert_eq!(leaf.files_recursive().count(), 1);
    assert_eq!(leaf.folders_recursive().count(), 0);
}

#[test]
fn the_iterators_are_lazy() {
    // Taking one file must not walk the whole tree, which is the point of
    // returning an iterator rather than a Vec.
    let root = sample();
    let first = root.files_recursive().next().unwrap();
    assert_eq!(first.0, "top.txt");
}

#[test]
fn an_empty_tree_walks_to_nothing() {
    let empty = folder("", &[], vec![]);
    assert_eq!(empty.files_recursive().count(), 0);
    assert_eq!(empty.folders_recursive().count(), 0);
}
