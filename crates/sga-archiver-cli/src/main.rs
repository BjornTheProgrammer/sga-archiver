use std::{
    fs,
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::{Parser, Subcommand};
use relic_chunky::{
    chunky::ChunkFile,
    decompile::DecompiledReflect,
    reflect::ReflectFile,
    rgd::{RelicGameData, game_data_to_xml},
};
use sga::{extract_all, read_header};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a mod source directory into an `.sga` archive.
    Pack {
        /// Mod source directory.
        input: PathBuf,
        /// Output `.sga` archive.
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
    },
    /// Unpack an `.sga` archive into a directory.
    Unpack {
        /// Input `.sga` archive.
        input: PathBuf,
        /// Output directory.
        #[arg(short, long, value_name = "DIR")]
        output: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Pack { input, output } => {
            sga::compile(&input, &output)?;
            println!("Packed {} into {}", input.display(), output.display());
        }
        Command::Unpack { input, output } => {
            let written = extract_all(&input, &output)?;
            decode_rgd_files(&written);
            decode_reflect_files(&written);
            write_aoe4mod(&input, &output, &written)?;
        }
    }
    Ok(())
}

fn write_aoe4mod(input: &Path, output: &Path, written_files: &[PathBuf]) -> Result<()> {
    let header = read_header(input)?;
    let guid = format_guid(&header.name);

    let locdb = written_files
        .iter()
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("locdb")));

    let (locdb_rel, mod_name) = match locdb {
        Some(path) => {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&header.name)
                .to_string();
            let rel = path
                .strip_prefix(output)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('/', "\\");
            (rel, name)
        }
        None => (String::new(), header.name.clone()),
    };

    let mut xml = String::new();
    xml.push_str("\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n");
    xml.push_str("<Mod xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns=\"http://schemas.datacontract.org/2004/07/Essence.Editor.Modding\">\r\n");
    xml.push_str("\t<DataGenericPath>assets</DataGenericPath>\r\n");
    xml.push_str("\t<DataIntermediatePath>cache</DataIntermediatePath>\r\n");
    xml.push_str(&format!("\t<ID>{guid}</ID>\r\n"));
    if !locdb_rel.is_empty() {
        xml.push_str(&format!("\t<LocDBPath>{locdb_rel}</LocDBPath>\r\n"));
    }
    xml.push_str("\t<Type>Extension</Type>\r\n");
    xml.push_str("</Mod>");

    let out_file = output.join(format!("{mod_name}.aoe4mod"));
    fs::write(&out_file, xml)?;
    println!("Wrote {}", out_file.display());

    Ok(())
}

fn format_guid(raw: &str) -> String {
    if raw.len() == 32 && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!(
            "{}-{}-{}-{}-{}",
            &raw[0..8],
            &raw[8..12],
            &raw[12..16],
            &raw[16..20],
            &raw[20..32]
        )
    } else {
        raw.to_string()
    }
}

/// Decompiles extracted reflection files (`.bin`/`.rdo`) to a `.txt` report and
/// a `.rdo` source alongside each.
fn decode_reflect_files(written_files: &[PathBuf]) {
    let mut decompiled = 0;
    for path in written_files.iter().filter(|p| {
        p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("rdo") || e.eq_ignore_ascii_case("bin"))
    }) {
        match decode_reflect_file(path) {
            Ok(true) => decompiled += 1,
            Ok(false) => {}
            Err(error) => eprintln!("failed to decompile '{}': {error:#}", path.display()),
        }
    }
    if decompiled > 0 {
        println!("Decompiled {decompiled} reflection file(s) to txt");
    }
}

fn decode_reflect_file(path: &Path) -> Result<bool> {
    let file = fs::File::open(path)?;
    let mut chunk_file = match ChunkFile::parse(BufReader::new(file)) {
        Ok(chunk_file) => chunk_file,
        Err(_) => return Ok(false),
    };

    match ReflectFile::parse(&mut chunk_file) {
        Some(reflect) => {
            fs::write(path.with_extension("txt"), reflect.to_report())?;
            let file = fs::File::open(path)?;
            let mut chunk_file = ChunkFile::parse(BufReader::new(file))?;
            if let Some(decompiled) = DecompiledReflect::parse(&mut chunk_file) {
                fs::write(path.with_extension("rdo"), decompiled.to_rdo_xml())?;
            }
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Decodes extracted `.rgd` game-data files to `.xml` alongside each.
fn decode_rgd_files(written_files: &[PathBuf]) {
    let rgd_paths: Vec<&PathBuf> = written_files
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("rgd")))
        .collect();
    if rgd_paths.is_empty() {
        return;
    }

    let mut failed = 0;
    for path in &rgd_paths {
        if let Err(error) = decode_rgd_file(path) {
            eprintln!("failed to decode '{}': {error:#}", path.display());
            failed += 1;
        }
    }
    println!("Decoded {} of {} .rgd files to xml", rgd_paths.len() - failed, rgd_paths.len());
}

fn decode_rgd_file(path: &Path) -> Result<()> {
    let file = fs::File::open(path)?;
    let mut chunk_file = ChunkFile::parse(BufReader::new(file))?;
    let nodes = RelicGameData::parse(&mut chunk_file)?;
    fs::write(path.with_extension("xml"), game_data_to_xml(&nodes)?)?;
    Ok(())
}
