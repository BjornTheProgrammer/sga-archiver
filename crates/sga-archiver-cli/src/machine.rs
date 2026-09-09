//! Machine-readable commands: one JSON object on stdout, nothing else.
//!
//! These exist so other tools can drive the archiver without parsing prose.
//! Every result carries a `schema` naming its shape and a `tool` block naming
//! the version that produced it. Failures are `Err`, which `main` turns into a
//! message on stderr and a nonzero exit; stdout never carries a partial result.
//!
//! Reads go through the lazy [`sga::ArchiveReader`] so listing or extracting
//! from a multi-gigabyte base-game archive touches only the bytes it needs.
//! Writes (repack, graft, replace, compile) load the archive in full, which is
//! what the byte-exact writer needs and what mod-sized archives afford.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use relic_chunky::{
    container::Chunky,
    hash::dictionary_hash,
    rgd::{RelicGameData, game_data_to_xml},
    rgd_patch,
};
use serde_json::{Value, json};
use sga::{Archive, ArchiveIndex, ArchiveReader, IndexFile, Verification};

const TOOL_NAME: &str = env!("CARGO_PKG_NAME");
const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

fn envelope(schema: &str, body: Value) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("schema".into(), json!(format!("{TOOL_NAME}.{schema}/v1")));
    out.insert(
        "tool".into(),
        json!({"name": TOOL_NAME, "version": TOOL_VERSION}),
    );
    if let Value::Object(fields) = body {
        out.extend(fields);
    }
    Value::Object(out)
}

/// Archive paths use `\` on the wire; reports use `/` so consumers on any
/// platform can join them.
fn report_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn normalized(path: &str) -> String {
    report_path(path).to_lowercase()
}

fn debug_name<T: std::fmt::Debug>(value: &T) -> String {
    format!("{value:?}")
}

fn index_summary(index: &ArchiveIndex, path: &Path) -> Value {
    let header = &index.header;
    json!({
        "path": path,
        "name": header.name,
        "version": header.version,
        "product": header.product,
        "block_size": header.block_size,
        "header_encryption": debug_name(&header.header_encryption_type),
        "signature_present": header.signature.iter().any(|&b| b != 0),
        "toc_count": index.tocs.len(),
        "folder_count": index.folder_count(),
        "file_count": index.file_count(),
        "tocs": index.tocs.iter().map(|toc| json!({"alias": toc.alias, "name": toc.name})).collect::<Vec<_>>(),
        "members": index.files().map(|(toc, path, _)| json!({"toc": toc.alias, "path": report_path(&path)})).collect::<Vec<_>>(),
    })
}

fn archive_summary(archive: &Archive) -> Value {
    json!({
        "name": archive.name,
        "version": archive.version,
        "product": archive.product,
        "block_size": archive.block_size,
        "header_encryption": debug_name(&archive.header_encryption_type),
        "signature_present": archive.signature.iter().any(|&b| b != 0),
        "toc_count": archive.tocs.len(),
        "folder_count": archive.tocs.iter().map(|toc| toc.root.folders_recursive().count()).sum::<usize>(),
        "file_count": archive.tocs.iter().map(|toc| toc.root.files_recursive().count()).sum::<usize>(),
        "tocs": archive.tocs.iter().map(|toc| json!({"alias": toc.alias, "name": toc.name})).collect::<Vec<_>>(),
        "members": archive.files().map(|(toc, path, _)| json!({"toc": toc.alias, "path": report_path(&path)})).collect::<Vec<_>>(),
    })
}

fn member_entry(toc: &str, path: &str, file: &IndexFile) -> Value {
    json!({
        "toc": toc,
        "path": report_path(path),
        "size": file.uncompressed_size,
        "stored_size": file.stored_size,
        "storage": debug_name(&file.storage_type),
        "encryption": debug_name(&file.encryption_type),
        "verification": debug_name(&file.verification_type),
        "crc": file.crc,
    })
}

fn verification_report(report: &Verification) -> Value {
    let mismatches = |items: &[sga::Mismatch]| {
        items
            .iter()
            .map(|m| json!({"toc": m.toc, "path": report_path(&m.path)}))
            .collect::<Vec<_>>()
    };
    json!({
        "verified": report.verified(),
        "checked_files": report.checked_files,
        "crc_mismatches": mismatches(&report.crc_mismatches),
        "sha1_mismatches": mismatches(&report.sha1_mismatches),
        "unhashed_files": report.unhashed_files,
        "scope": "stored bytes against recorded CRC32 and SHA1 block hashes; the header signature is not validated",
    })
}

fn open(input: &Path) -> Result<ArchiveReader<BufReader<File>>> {
    sga::open(input).with_context(|| format!("failed to read SGA {}", input.display()))
}

fn refuse_existing(output: &Path) -> Result<()> {
    if output.exists() {
        bail!("refusing to overwrite output {}", output.display());
    }
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

/// A member selected by exact path, or failing that by case-insensitive path
/// with either separator. Returns the TOC alias, the `\`-path and the entry.
fn find_member(index: &ArchiveIndex, member: &str) -> Option<(String, String, IndexFile)> {
    if let Some((toc, file)) = index.file(member) {
        let path = index
            .files()
            .find(|(t, _, f)| std::ptr::eq(*f, file) && std::ptr::eq(*t, toc))
            .map(|(_, path, _)| path)
            .unwrap_or_else(|| member.replace('/', "\\"));
        return Some((toc.alias.clone(), path, file.clone()));
    }
    let wanted = normalized(member.trim_matches(['/', '\\']));
    index
        .files()
        .find(|(_, path, _)| normalized(path) == wanted)
        .map(|(toc, path, file)| (toc.alias.clone(), path, file.clone()))
}

/// Rejects archive paths that would escape an extraction directory.
fn safe_relative(path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.split(['\\', '/']) {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." || component.contains(':') {
            bail!("refusing archive path that escapes the output directory: {path}");
        }
        out.push(component);
    }
    if out.as_os_str().is_empty() {
        bail!("archive path is empty");
    }
    Ok(out)
}

pub fn inspect(input: &Path) -> Result<Value> {
    let reader = open(input)?;
    Ok(envelope(
        "inspect",
        json!({"archive": index_summary(reader.index(), input)}),
    ))
}

pub fn list(input: &Path, filter: Option<&str>, verify: bool) -> Result<Value> {
    let mut reader = open(input)?;
    let needle = filter.map(|f| f.to_lowercase());
    let members = reader
        .index()
        .files()
        .filter(|(_, path, _)| {
            needle
                .as_deref()
                .is_none_or(|needle| normalized(path).contains(needle))
        })
        .map(|(toc, path, file)| member_entry(&toc.alias, &path, file))
        .collect::<Vec<_>>();
    let verification = if verify {
        Some(verification_report(&reader.verify()?))
    } else {
        None
    };
    let mut archive = index_summary(reader.index(), input);
    if let Value::Object(fields) = &mut archive {
        fields.remove("members");
        fields.insert("verification".into(), json!(verification));
    }
    Ok(envelope(
        "list",
        json!({
            "archive": archive,
            "filter": filter,
            "member_count": members.len(),
            "members": members,
        }),
    ))
}

pub fn extract_member(input: &Path, member: &str, output: &Path) -> Result<Value> {
    if member.trim().is_empty() {
        bail!("archive member must not be empty");
    }
    refuse_existing(output)?;
    let mut reader = open(input)?;
    let (toc, path, file) = find_member(reader.index(), member)
        .with_context(|| format!("archive member not found: {member}"))?;
    let data = reader.read(&file)?;
    fs::write(output, &data).with_context(|| format!("failed to write {}", output.display()))?;
    Ok(envelope(
        "extract-member",
        json!({
            "input": input,
            "member": member,
            "toc": toc,
            "path": report_path(&path),
            "output": output,
            "size": data.len(),
        }),
    ))
}

pub fn extract(
    input: &Path,
    output: &Path,
    filter: Option<&str>,
    members: &[String],
) -> Result<Value> {
    if filter.is_none() && members.is_empty() {
        bail!("extract needs --filter or at least one --member; use `unpack` for a whole archive");
    }
    let needle = filter.map(|f| f.to_lowercase());
    let wanted = members
        .iter()
        .map(|m| normalized(m.trim_matches(['/', '\\'])))
        .collect::<BTreeSet<_>>();
    fs::create_dir_all(output).with_context(|| format!("failed to create {}", output.display()))?;

    let mut reader = open(input)?;
    let selected = reader
        .index()
        .files()
        .filter(|(_, path, _)| {
            let key = normalized(path);
            needle.as_deref().is_some_and(|needle| key.contains(needle)) || wanted.contains(&key)
        })
        .map(|(toc, path, file)| (toc.alias.clone(), path, file.clone()))
        .collect::<Vec<_>>();

    let mut written = Vec::new();
    let mut duplicates = Vec::new();
    let mut encrypted = Vec::new();
    let mut seen = BTreeSet::new();
    for (toc, path, file) in selected {
        let key = normalized(&path);
        if !seen.insert(key) {
            duplicates.push(report_path(&path));
            continue;
        }
        if file.encryption_type.is_encrypted() {
            encrypted.push(json!({
                "toc": toc,
                "path": report_path(&path),
                "encryption": debug_name(&file.encryption_type),
            }));
            continue;
        }
        let target = output.join(safe_relative(&path)?);
        if target.exists() {
            bail!("refusing to overwrite existing file {}", target.display());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let data = reader.read(&file)?;
        fs::write(&target, &data)
            .with_context(|| format!("failed to write {}", target.display()))?;
        written.push(json!({
            "toc": toc,
            "path": report_path(&path),
            "size": data.len(),
            "output": target,
        }));
    }
    let missing = wanted
        .iter()
        .filter(|key| !seen.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!("archive members not found: {}", missing.join(", "));
    }
    Ok(envelope(
        "extract",
        json!({
            "input": input,
            "output": output,
            "filter": filter,
            "members": members,
            "written_count": written.len(),
            "written": written,
            "duplicates": duplicates,
            "encrypted": encrypted,
        }),
    ))
}

pub fn decode_rgd(input: &Path, output: &Path) -> Result<Value> {
    if input == output {
        bail!("input and output must be different paths");
    }
    refuse_existing(output)?;
    let file = File::open(input).with_context(|| format!("failed to open {}", input.display()))?;
    let chunky = Chunky::read(&mut BufReader::new(file))
        .with_context(|| format!("failed to parse RGD container {}", input.display()))?;
    let nodes = RelicGameData::parse(&chunky)
        .with_context(|| format!("failed to parse RGD data {}", input.display()))?;
    let xml = game_data_to_xml(&nodes).context("failed to serialize RGD XML")?;
    fs::write(output, xml).with_context(|| format!("failed to write {}", output.display()))?;
    Ok(envelope(
        "decode-rgd",
        json!({"input": input, "output": output}),
    ))
}

fn write_patched(input: &Path, output: &Path, bytes: &[u8]) -> Result<()> {
    if input == output {
        bail!("input and output must be different paths");
    }
    refuse_existing(output)?;
    fs::write(output, bytes).with_context(|| format!("failed to write {}", output.display()))
}

pub fn patch_rgd_cstring(
    input: &Path,
    output: &Path,
    old: &str,
    new: &str,
    expected_count: usize,
) -> Result<Value> {
    if input == output {
        bail!("input and output must be different paths");
    }
    let input_bytes =
        fs::read(input).with_context(|| format!("failed to read {}", input.display()))?;
    let (bytes, report) =
        rgd_patch::patch_cstring_bytes(&input_bytes, old.as_bytes(), new.as_bytes())?;
    if report.replacements != expected_count {
        bail!(
            "expected {expected_count} CString replacements, found {}",
            report.replacements
        );
    }
    write_patched(input, output, &bytes)?;
    Ok(envelope(
        "patch-rgd-cstring",
        json!({
            "input": input,
            "output": output,
            "old": old,
            "new": new,
            "replacements": report.replacements,
            "original_aegd_size": report.original_aegd_size,
            "patched_aegd_size": report.patched_aegd_size,
            "size": bytes.len(),
        }),
    ))
}

pub fn patch_rgd_f32(
    input: &Path,
    output: &Path,
    key: &str,
    old: f32,
    new: f32,
    expected_count: usize,
) -> Result<Value> {
    if input == output {
        bail!("input and output must be different paths");
    }
    if key.is_empty() || key.contains('\0') {
        bail!("Float key must be non-empty and contain no NUL bytes");
    }
    let key_hash = dictionary_hash(key);
    let input_bytes =
        fs::read(input).with_context(|| format!("failed to read {}", input.display()))?;
    let (bytes, report) = rgd_patch::patch_f32_bytes(&input_bytes, key_hash, old, new)?;
    if report.replacements != expected_count {
        bail!(
            "expected {expected_count} Float replacements, found {}",
            report.replacements
        );
    }
    write_patched(input, output, &bytes)?;
    Ok(envelope(
        "patch-rgd-f32",
        json!({
            "input": input,
            "output": output,
            "key": key,
            "key_hash": key_hash,
            "old": old,
            "new": new,
            "replacements": report.replacements,
            "original_aegd_size": report.original_aegd_size,
            "patched_aegd_size": report.patched_aegd_size,
            "size": bytes.len(),
        }),
    ))
}

fn read_eager(path: &Path, role: &str) -> Result<Archive> {
    sga::read_archive(path).with_context(|| format!("failed to read {role} SGA {}", path.display()))
}

fn write_eager(archive: &mut Archive, output: &Path) -> Result<()> {
    // A tree read from an archive the editor wrote is already in order;
    // anything grafted or replaced here was inserted in order. Sorting once
    // more before writing costs nothing and guarantees it for a tree that
    // arrived out of order, which the game answers with a missing file.
    archive.sort_directories();
    let mut writer =
        File::create(output).with_context(|| format!("failed to create {}", output.display()))?;
    archive
        .write(&mut writer)
        .with_context(|| format!("failed to write SGA {}", output.display()))
}

pub fn repack(input: &Path, output: &Path) -> Result<Value> {
    if input == output {
        bail!("input and output must be different paths");
    }
    refuse_existing(output)?;
    let mut archive = read_eager(input, "input")?;
    write_eager(&mut archive, output)?;
    Ok(envelope(
        "repack",
        json!({"input": input, "output": output, "archive": archive_summary(&archive)}),
    ))
}

pub fn compile_project(project: &Path, output: &Path) -> Result<Value> {
    if !project.is_dir() {
        bail!("project source is not a directory: {}", project.display());
    }
    refuse_existing(output)?;
    sga::compile(project, output).with_context(|| {
        format!(
            "failed to compile project {} to {}",
            project.display(),
            output.display()
        )
    })?;
    let reader = open(output)?;
    Ok(envelope(
        "compile-project",
        json!({
            "project": project,
            "output": output,
            "archive": index_summary(reader.index(), output),
        }),
    ))
}

/// The alias of the TOC holding `member`, matched like [`find_member`].
fn member_toc_alias(archive: &Archive, member: &str) -> Result<String> {
    let wanted = normalized(member.trim_matches(['/', '\\']));
    archive
        .files()
        .find(|(_, path, _)| normalized(path) == wanted)
        .map(|(toc, _, _)| toc.alias.clone())
        .with_context(|| format!("archive member not found while resolving TOC: {member}"))
}

fn donor_bytes(donor: &Archive, member: &str) -> Result<Vec<u8>> {
    let wanted = normalized(member.trim_matches(['/', '\\']));
    let (_, _, file) = donor
        .files()
        .find(|(_, path, _)| normalized(path) == wanted)
        .with_context(|| format!("donor member not found: {member}"))?;
    file.decoded()
}

pub fn graft(base: &Path, donor: &Path, output: &Path, members: &[String]) -> Result<Value> {
    if members.is_empty() {
        bail!("graft requires at least one archive member");
    }
    if base == output || donor == output {
        bail!("output must differ from both input archives");
    }
    refuse_existing(output)?;
    let mut archive = read_eager(base, "base")?;
    let donor_archive = read_eager(donor, "donor")?;
    for member in members {
        let toc_alias = member_toc_alias(&donor_archive, member)?;
        let data = donor_bytes(&donor_archive, member)?;
        archive.upsert_stored_in(&toc_alias, member, data);
    }
    write_eager(&mut archive, output)?;
    Ok(envelope(
        "graft",
        json!({
            "base": base,
            "donor": donor,
            "output": output,
            "grafted_members": members,
            "archive": archive_summary(&archive),
        }),
    ))
}

pub fn graft_as(
    base: &Path,
    donor: &Path,
    output: &Path,
    mappings: &[(String, String)],
) -> Result<Value> {
    if mappings.is_empty() {
        bail!("graft-as requires at least one source=target mapping");
    }
    if base == output || donor == output {
        bail!("output must differ from both input archives");
    }
    refuse_existing(output)?;
    let mut archive = read_eager(base, "base")?;
    let donor_archive = read_eager(donor, "donor")?;
    for (source, target) in mappings {
        let toc_alias = member_toc_alias(&donor_archive, source)?;
        let data = donor_bytes(&donor_archive, source)?;
        archive.upsert_stored_in(&toc_alias, target, data);
    }
    write_eager(&mut archive, output)?;
    Ok(envelope(
        "graft-as",
        json!({
            "base": base,
            "donor": donor,
            "output": output,
            "grafted_mappings": mappings.iter().map(|(source, target)| json!({"source": source, "target": target})).collect::<Vec<_>>(),
            "archive": archive_summary(&archive),
        }),
    ))
}

pub fn replace_member(base: &Path, payload: &Path, output: &Path, member: &str) -> Result<Value> {
    if member.trim().is_empty() {
        bail!("archive member must not be empty");
    }
    if base == output || payload == output {
        bail!("output must differ from both input files");
    }
    refuse_existing(output)?;
    let mut archive = read_eager(base, "base")?;
    let toc_alias = member_toc_alias(&archive, member)?;
    let data = fs::read(payload)
        .with_context(|| format!("failed to read payload {}", payload.display()))?;
    let size = data.len();
    archive.upsert_stored_in(&toc_alias, member, data);
    write_eager(&mut archive, output)?;
    Ok(envelope(
        "replace-member",
        json!({
            "base": base,
            "payload": {"path": payload, "size": size},
            "output": output,
            "replaced_member": member,
            "archive": archive_summary(&archive),
        }),
    ))
}

/// Parses `source=target` pairs for `graft-as`.
pub fn parse_mappings(values: &[String]) -> Result<Vec<(String, String)>> {
    values
        .iter()
        .map(|value| {
            value
                .split_once('=')
                .filter(|(source, target)| !source.is_empty() && !target.is_empty())
                .map(|(source, target)| (source.to_owned(), target.to_owned()))
                .with_context(|| format!("invalid graft-as mapping: {value}"))
        })
        .collect()
}
