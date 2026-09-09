mod machine;

use std::{
    fs,
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::{Parser, Subcommand};
use relic_chunky::{
    container::Chunky,
    decompile::DecompiledReflect,
    rgd::{RelicGameData, game_data_to_xml},
};
use sga::{extract_all, read_header};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// `pack` and `unpack` talk to a person. Every other command prints exactly
/// one JSON object on stdout so another program can drive the archiver; see
/// [`machine`].
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
    /// Describe an archive and name its members, without reading their bytes (JSON).
    Inspect {
        /// Input `.sga` archive.
        input: PathBuf,
    },
    /// List members with size, storage, encryption and verification details (JSON).
    List {
        /// Input `.sga` archive.
        input: PathBuf,
        /// Only members whose path contains this text, compared case-insensitively.
        #[arg(long)]
        filter: Option<String>,
        /// Also check every member's stored bytes against its recorded CRC and
        /// SHA1 block hashes. Reads the whole archive once.
        #[arg(long)]
        verify: bool,
    },
    /// Write one member's decoded bytes to a file (JSON).
    ExtractMember {
        /// Input `.sga` archive.
        input: PathBuf,
        /// Archive path of the member, with either separator.
        member: String,
        /// Output file; must not already exist.
        output: PathBuf,
    },
    /// Write selected members under a directory, keeping their archive paths (JSON).
    Extract {
        /// Input `.sga` archive.
        input: PathBuf,
        /// Output directory.
        output: PathBuf,
        /// Members whose path contains this text, compared case-insensitively.
        #[arg(long)]
        filter: Option<String>,
        /// An exact member to extract; repeatable.
        #[arg(long = "member")]
        members: Vec<String>,
    },
    /// Decode a compiled `.rgd` to XML (JSON).
    DecodeRgd { input: PathBuf, output: PathBuf },
    /// Replace every CString equal to OLD with NEW inside an `.rgd` (JSON).
    PatchRgdCstring {
        input: PathBuf,
        output: PathBuf,
        old: String,
        new: String,
        /// The patch is refused unless exactly this many values change.
        expected_count: usize,
    },
    /// Replace every Float under KEY equal to OLD with NEW inside an `.rgd` (JSON).
    PatchRgdF32 {
        input: PathBuf,
        output: PathBuf,
        key: String,
        old: f32,
        new: f32,
        /// The patch is refused unless exactly this many values change.
        expected_count: usize,
    },
    /// Compile a mod source directory and describe the result (JSON).
    CompileProject { project: PathBuf, output: PathBuf },
    /// Read an archive and write it back (JSON).
    Repack { input: PathBuf, output: PathBuf },
    /// Copy members from a donor archive into a base archive (JSON).
    Graft {
        base: PathBuf,
        donor: PathBuf,
        output: PathBuf,
        #[arg(required = true)]
        members: Vec<String>,
    },
    /// Copy donor members into a base archive under new paths (JSON).
    GraftAs {
        base: PathBuf,
        donor: PathBuf,
        output: PathBuf,
        /// `source=target` pairs.
        #[arg(required = true)]
        mappings: Vec<String>,
    },
    /// Replace one existing member's bytes with a file's contents (JSON).
    ReplaceMember {
        base: PathBuf,
        payload: PathBuf,
        output: PathBuf,
        member: String,
    },
}

fn main() -> Result<()> {
    let value = match Cli::parse().command {
        Command::Pack { input, output } => {
            sga::compile(&input, &output)?;
            println!("Packed {} into {}", input.display(), output.display());
            return Ok(());
        }
        Command::Unpack { input, output } => {
            let written = extract_all(&input, &output)?;
            decode_rgd_files(&written);
            decode_reflect_files(&written);
            write_aoe4mod(&input, &output, &written)?;
            return Ok(());
        }
        Command::Inspect { input } => machine::inspect(&input)?,
        Command::List {
            input,
            filter,
            verify,
        } => machine::list(&input, filter.as_deref(), verify)?,
        Command::ExtractMember {
            input,
            member,
            output,
        } => machine::extract_member(&input, &member, &output)?,
        Command::Extract {
            input,
            output,
            filter,
            members,
        } => machine::extract(&input, &output, filter.as_deref(), &members)?,
        Command::DecodeRgd { input, output } => machine::decode_rgd(&input, &output)?,
        Command::PatchRgdCstring {
            input,
            output,
            old,
            new,
            expected_count,
        } => machine::patch_rgd_cstring(&input, &output, &old, &new, expected_count)?,
        Command::PatchRgdF32 {
            input,
            output,
            key,
            old,
            new,
            expected_count,
        } => machine::patch_rgd_f32(&input, &output, &key, old, new, expected_count)?,
        Command::CompileProject { project, output } => machine::compile_project(&project, &output)?,
        Command::Repack { input, output } => machine::repack(&input, &output)?,
        Command::Graft {
            base,
            donor,
            output,
            members,
        } => machine::graft(&base, &donor, &output, &members)?,
        Command::GraftAs {
            base,
            donor,
            output,
            mappings,
        } => machine::graft_as(&base, &donor, &output, &machine::parse_mappings(&mappings)?)?,
        Command::ReplaceMember {
            base,
            payload,
            output,
            member,
        } => machine::replace_member(&base, &payload, &output, &member)?,
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn write_aoe4mod(input: &Path, output: &Path, written_files: &[PathBuf]) -> Result<()> {
    let header = read_header(input)?;
    let guid = format_guid(&header.name);

    let locdb = written_files.iter().find(|p| {
        p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("locdb"))
    });

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
        println!("Decompiled {decompiled} reflection file(s) to rdo");
    }
}

fn decode_reflect_file(path: &Path) -> Result<bool> {
    let file = fs::File::open(path)?;
    let chunky = match Chunky::read(&mut BufReader::new(file)) {
        Ok(chunky) => chunky,
        Err(_) => return Ok(false),
    };

    match DecompiledReflect::parse(&chunky) {
        Some(decompiled) => {
            fs::write(path.with_extension("rdo"), decompiled.to_rdo_xml())?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Decodes extracted `.rgd` game-data files to `.xml` alongside each.
fn decode_rgd_files(written_files: &[PathBuf]) {
    let rgd_paths: Vec<&PathBuf> = written_files
        .iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rgd"))
        })
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
    println!(
        "Decoded {} of {} .rgd files to xml",
        rgd_paths.len() - failed,
        rgd_paths.len()
    );
}

fn decode_rgd_file(path: &Path) -> Result<()> {
    let file = fs::File::open(path)?;
    let chunky = Chunky::read(&mut BufReader::new(file))?;
    let nodes = RelicGameData::parse(&chunky)?;
    fs::write(path.with_extension("xml"), game_data_to_xml(&nodes)?)?;
    Ok(())
}
