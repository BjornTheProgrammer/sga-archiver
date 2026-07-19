use std::{
    fs,
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use relic_chunky::{
    chunky::ChunkFile,
    decompile::DecompiledReflect,
    reflect::ReflectFile,
    rgd::{RelicGameData, game_data_to_json, game_data_to_xml},
};
use sga::{extract_all, read_header};

/// Format to decode extracted .rgd files into.
#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum RgdFormat {
    Xml,
    Json,
    /// Leave .rgd files as they are.
    None,
}

impl RgdFormat {
    fn extension(&self) -> Option<&'static str> {
        match self {
            RgdFormat::Xml => Some("xml"),
            RgdFormat::Json => Some("json"),
            RgdFormat::None => None,
        }
    }
}

/// Whether to decompile reflection files (win-condition .rdo/.bin) into a
/// readable .txt report alongside them.
#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum ReflectFormat {
    /// Write a human-readable .txt report next to each reflection file.
    Text,
    /// Leave reflection files as they are.
    None,
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Input file path
    input: PathBuf,

    /// Output folder path
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,

    /// Format to decode extracted .rgd files into, written alongside the .rgd
    #[arg(long, value_enum, default_value_t = RgdFormat::Xml)]
    rgd_format: RgdFormat,

    /// Whether to decompile reflection files (win-condition .rdo/.bin) to .txt
    #[arg(long, value_enum, default_value_t = ReflectFormat::Text)]
    reflect_format: ReflectFormat,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let written_files = extract_all(&cli.input, &cli.output)?;
    decode_rgd_files(&written_files, cli.rgd_format)?;
    decode_reflect_files(&written_files, cli.reflect_format)?;
    write_aoe4mod(&cli.input, &cli.output, &written_files)?;

    Ok(())
}

/// Reconstructs the `<ModName>.aoe4mod` Content Editor project file at the
/// root of the extracted output, so a packed mod can be re-opened as an
/// editable project. Every field is recoverable from the archive:
///   * the mod GUID is the sga header's `name` field (a dash-less hex string),
///   * the loc-db path and the mod's display name come from the extracted
///     `locdb\*.locdb` file,
///   * the data/intermediate paths are the editor's fixed conventions.
fn write_aoe4mod(input: &Path, output: &Path, written_files: &[PathBuf]) -> Result<()> {
    let header = read_header(input)?;
    let guid = format_guid(&header.name);

    // The .locdb path (relative to the assets dir) doubles as the source of
    // the mod's display name (its file stem). Falls back to the archive name.
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
            // Editor stores this relative to the mod root, e.g. `locdb\Foo.locdb`.
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

/// Formats a 32-char dash-less hex GUID as `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
/// Returns the input unchanged if it is not exactly 32 hex characters.
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

/// Decompiles every reflection file (win-condition .rdo/.bin) among
/// `written_files` into a readable .txt report written next to it. Files that
/// are not reflection files (no RFTY chunks) are skipped silently; a file that
/// errors is reported but does not fail the extraction.
fn decode_reflect_files(written_files: &[PathBuf], format: ReflectFormat) -> Result<()> {
    if format == ReflectFormat::None {
        return Ok(());
    }

    // Only .rdo/.bin can be reflection files; probing every extracted file
    // would waste work opening textures, scripts, etc.
    let candidates: Vec<&PathBuf> = written_files
        .iter()
        .filter(|path| {
            path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("rdo") || ext.eq_ignore_ascii_case("bin")
            })
        })
        .collect();

    let mut decompiled = 0;
    for path in &candidates {
        match decode_reflect_file(path) {
            Ok(true) => decompiled += 1,
            Ok(false) => {}
            Err(error) => eprintln!("failed to decompile '{}': {error:#}", path.display()),
        }
    }

    if decompiled > 0 {
        println!("Decompiled {decompiled} reflection file(s) to txt");
    }

    Ok(())
}

/// Attempts to decompile one file as a reflection file. Returns `Ok(true)` if
/// it was a reflection file and a report was written, `Ok(false)` if it was
/// not a reflection file (nothing written).
fn decode_reflect_file(path: &Path) -> Result<bool> {
    let file = fs::File::open(path)?;
    let mut chunk_file = match ChunkFile::parse(BufReader::new(file)) {
        Ok(chunk_file) => chunk_file,
        // Not a chunky file at all (many .bin files are not): skip quietly.
        Err(_) => return Ok(false),
    };

    match ReflectFile::parse(&mut chunk_file) {
        Some(reflect) => {
            // Readable summary report...
            fs::write(path.with_extension("txt"), reflect.to_report())?;
            // ...and the editor-source .rdo reconstruction.
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

/// Decodes every .rgd file among `written_files`, writing the result next to it.
/// A file that fails to decode is reported but does not fail the extraction.
fn decode_rgd_files(written_files: &[PathBuf], format: RgdFormat) -> Result<()> {
    let Some(extension) = format.extension() else {
        return Ok(());
    };

    let rgd_paths: Vec<&PathBuf> = written_files
        .iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rgd"))
        })
        .collect();

    if rgd_paths.is_empty() {
        return Ok(());
    }

    let mut failed = 0;
    for path in &rgd_paths {
        if let Err(error) = decode_rgd_file(path, extension) {
            eprintln!("failed to decode '{}': {error:#}", path.display());
            failed += 1;
        }
    }

    println!(
        "Decoded {} of {} .rgd files to {extension}",
        rgd_paths.len() - failed,
        rgd_paths.len()
    );

    Ok(())
}

fn decode_rgd_file(path: &Path, extension: &str) -> Result<()> {
    let file = fs::File::open(path)?;
    let mut chunk_file = ChunkFile::parse(BufReader::new(file))?;
    let nodes = RelicGameData::parse(&mut chunk_file)?;

    let encoded = match extension {
        "json" => game_data_to_json(&nodes)?,
        _ => game_data_to_xml(&nodes)?,
    };

    fs::write(path.with_extension(extension), encoded)?;

    Ok(())
}
