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

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum RgdFormat {
    Xml,
    Json,
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

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum ReflectFormat {
    Text,
    None,
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    input: PathBuf,

    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,

    #[arg(long, value_enum, default_value_t = RgdFormat::Xml)]
    rgd_format: RgdFormat,

    #[arg(long, value_enum, default_value_t = ReflectFormat::Text)]
    reflect_format: ReflectFormat,

    #[arg(long)]
    compile: bool,

    #[arg(long)]
    dump_schema: bool,

    /// Extract reflection schema resources (`<RootType>.schema`) from the `.bin`
    /// files under the input directory, into the output directory.
    #[arg(long)]
    dump_schema_lib: bool,

    /// Compile a single PNG (input) into a Relic `.rrtex` texture (output).
    #[arg(long)]
    compile_texture: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.dump_schema {
        let mut registry = relic_chunky::reflect_type::SchemaRegistry::new();
        let scanned = registry.scan_dir(&cli.input)?;
        let json = serde_json::to_string_pretty(&registry)?;
        std::fs::write(&cli.output, json)?;
        println!(
            "Scanned {} reflection files, collected {} unique types -> {}",
            scanned,
            registry.types.len(),
            cli.output.display()
        );
        return Ok(());
    }

    if cli.dump_schema_lib {
        let count = dump_schema_lib(&cli.input, &cli.output)?;
        println!("Wrote {count} schema resource(s) -> {}", cli.output.display());
        return Ok(());
    }

    if cli.compile_texture {
        let png = fs::read(&cli.input)?;
        let name = cli.input.file_stem().and_then(|s| s.to_str()).unwrap_or("texture");
        let rrtex = relic_chunky::texture::compile_texture(&png, name)?;
        fs::write(&cli.output, &rrtex)?;
        println!(
            "Compiled {} into {} ({} bytes)",
            cli.input.display(),
            cli.output.display(),
            rrtex.len()
        );
        return Ok(());
    }

    if cli.compile {
        sga::compile(&cli.input, &cli.output)?;
        println!(
            "Compiled {} into {}",
            cli.input.display(),
            cli.output.display()
        );
        return Ok(());
    }

    let written_files = extract_all(&cli.input, &cli.output)?;
    decode_rgd_files(&written_files, cli.rgd_format)?;
    decode_reflect_files(&written_files, cli.reflect_format)?;
    write_aoe4mod(&cli.input, &cli.output, &written_files)?;

    Ok(())
}

/// Walks `input` for reflection `.bin` files and writes one
/// `<RootType>.schema` resource per distinct root type into `out_dir`.
fn dump_schema_lib(input: &Path, out_dir: &Path) -> Result<usize> {
    fs::create_dir_all(out_dir)?;
    let mut written = 0;
    let mut stack = vec![input.to_path_buf()];
    let mut seen = std::collections::HashSet::new();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bin")) {
                continue;
            }
            let bytes = fs::read(&path)?;
            let Ok((root_type, schema)) = relic_chunky::reflect_write::extract_schema(&bytes) else {
                continue;
            };
            let file_name: String = root_type
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            if seen.insert(file_name.clone()) {
                fs::write(out_dir.join(format!("{file_name}.schema")), &schema)?;
                println!("  {} -> {file_name}.schema ({} bytes)", root_type, schema.len());
                written += 1;
            }
        }
    }
    Ok(written)
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

fn decode_reflect_files(written_files: &[PathBuf], format: ReflectFormat) -> Result<()> {
    if format == ReflectFormat::None {
        return Ok(());
    }

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
