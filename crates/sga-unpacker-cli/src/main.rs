use std::{
    fs,
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use relic_chunky::{
    chunky::ChunkFile,
    rgd::{RelicGameData, game_data_to_json, game_data_to_xml},
};
use sga::extract_all;

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let written_files = extract_all(cli.input, cli.output)?;
    decode_rgd_files(&written_files, cli.rgd_format)?;

    Ok(())
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
