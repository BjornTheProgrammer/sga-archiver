use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::Result;

mod archive;
mod build;
pub mod entries;
/// Former name of [`entries`], kept so the rename is not a breaking change.
#[deprecated(since = "0.2.1", note = "misspelling; use `entries`")]
pub use crate::entries as entires;
pub mod localization;
pub(crate) mod utils;

pub use archive::{Archive, FileEntry, Folder, Toc, TocLayout};

pub fn read_header<P: AsRef<Path>>(sga_file: P) -> Result<entries::SgaHeader> {
    let mut sga_file = BufReader::new(File::open(sga_file)?);
    Ok(entries::SgaHeader::parse(&mut sga_file)?)
}

pub fn read_archive<P: AsRef<Path>>(sga_file: P) -> Result<Archive> {
    let mut reader = BufReader::new(File::open(sga_file)?);
    Archive::read(&mut reader)
}

pub fn extract_all<P: AsRef<Path>>(sga_file: P, out_path: P) -> Result<Vec<PathBuf>> {
    let mut reader = BufReader::new(File::open(sga_file)?);
    let archive = Archive::read(&mut reader)?;
    archive.extract_to(out_path)
}

pub fn pack_dir<P: AsRef<Path>, Q: AsRef<Path>>(name: &str, dir: P, out_path: Q) -> Result<()> {
    let archive = build::from_dir(name, dir)?;
    let mut writer = File::create(out_path)?;
    archive.write(&mut writer)?;
    Ok(())
}

pub fn compile<P: AsRef<Path>, Q: AsRef<Path>>(source_dir: P, out_path: Q) -> Result<()> {
    let archive = build::compile_project(source_dir)?;
    let mut writer = File::create(out_path)?;
    archive.write(&mut writer)?;
    Ok(())
}
