use std::{fs::File, io::BufReader, path::{Path, PathBuf}};

use anyhow::Result;

mod archive;
pub mod entires;
pub(crate) mod utils;

pub use archive::{Archive, FileEntry, Folder, Toc};

pub fn read_header<P: AsRef<Path>>(sga_file: P) -> Result<entires::SgaHeader> {
    let mut sga_file = BufReader::new(File::open(sga_file)?);
    Ok(entires::SgaHeader::parse(&mut sga_file)?)
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
    let archive = Archive::from_dir(name, dir)?;
    let mut writer = File::create(out_path)?;
    archive.write(&mut writer)?;
    Ok(())
}

pub fn compile<P: AsRef<Path>, Q: AsRef<Path>>(source_dir: P, out_path: Q) -> Result<()> {
    let archive = Archive::compile_project(source_dir)?;
    let mut writer = File::create(out_path)?;
    archive.write(&mut writer)?;
    Ok(())
}
