use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::archive::{
    Archive, DEFAULT_BLOCK_SIZE, FileEntry, Folder, Toc, TocLayout, encode, get_or_create_toc,
    insert_file, insert_or_replace, stored_file,
};
use crate::entries::{FileEncryptionType, FileStorageType, FileVerificationType, HeaderReserved};
use flate2::Crc;

pub fn from_dir<P: AsRef<Path>>(name: &str, dir: P) -> Result<Archive> {
    let root = build_from_dir(dir.as_ref())?;
    Ok(Archive {
        name: name.to_string(),
        version: 11,
        product: 0,
        block_size: DEFAULT_BLOCK_SIZE,
        header_encryption_type: FileEncryptionType::None,
        signature: [0u8; 256],
        header_reserved: HeaderReserved::default(),
        layout: TocLayout::Modern,
        tocs: vec![Toc {
            alias: "data".to_string(),
            name: "data".to_string(),
            root,
        }],
    })
}

pub fn compile_project<P: AsRef<Path>>(source_dir: P) -> Result<Archive> {
    let source_dir = source_dir.as_ref();
    let name = read_guid(source_dir)?;
    let mut tocs = Vec::new();

    let mut data_root = Folder {
        name: String::new(),
        folders: Vec::new(),
        files: Vec::new(),
    };
    let assets = source_dir.join("assets");
    if assets.is_dir() {
        add_source_files(
            &mut data_root,
            &assets,
            &assets,
            Some("scar"),
            &FileStorageType::BufferCompress,
            &identity_transform,
        )?;
    }
    let prebuilt_data = source_dir.join("prebuilt").join("data");
    if prebuilt_data.is_dir() {
        add_source_files(
            &mut data_root,
            &prebuilt_data,
            &prebuilt_data,
            None,
            &FileStorageType::Store,
            &identity_transform,
        )?;
    }
    if !folder_is_empty(&data_root) {
        tocs.push(Toc {
            alias: "data".to_string(),
            name: "data".to_string(),
            root: data_root,
        });
    }

    for alias in ["info", "locale"] {
        let base = source_dir.join("prebuilt").join(alias);
        if base.is_dir() {
            let mut root = Folder {
                name: String::new(),
                folders: Vec::new(),
                files: Vec::new(),
            };
            add_source_files(
                &mut root,
                &base,
                &base,
                None,
                &FileStorageType::Store,
                &identity_transform,
            )?;
            if !folder_is_empty(&root) {
                tocs.push(Toc {
                    alias: alias.to_string(),
                    name: alias.to_string(),
                    root,
                });
            }
        }
    }

    if assets.is_dir() {
        add_reflection_bins(&mut tocs, &assets)?;
        add_texture_bins(&mut tocs, &assets)?;
        add_attribute_rgds(&mut tocs, &assets)?;
        add_localization(&mut tocs, &assets)?;
    }

    Ok(Archive {
        name,
        version: 11,
        product: 0,
        block_size: DEFAULT_BLOCK_SIZE,
        header_encryption_type: FileEncryptionType::None,
        signature: [0u8; 256],
        header_reserved: HeaderReserved::default(),
        layout: TocLayout::Modern,
        tocs,
    })
}

/// A `.burnproj` `BurnRule`: which source globs compile into which TOC (alias).
struct BurnRule {
    alias: String,
    includes: Vec<String>,
}

/// Compiles every source file under `assets` selected by a rule for `burner`
/// into the rule's TOC. `source_ext` filters the sources; the output keeps the
/// same relative directory with a lower-cased `<stem>.<out_ext>` name (matching
/// the editor). `compile` turns `(path, stem)` into the output bytes.
fn add_burned_files(
    tocs: &mut Vec<Toc>,
    assets: &Path,
    burner: &str,
    source_ext: &str,
    out_ext: &str,
    compile: impl Fn(&Path, &str) -> Result<Vec<u8>>,
) -> Result<()> {
    let Some(burnproj) = find_burnproj(assets) else {
        return Ok(());
    };
    let rules = parse_burn_rules(&std::fs::read_to_string(&burnproj)?, burner);
    if rules.is_empty() {
        return Ok(());
    }

    let mut stack = vec![assets.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(source_ext))
            {
                continue;
            }
            let rel = path.strip_prefix(assets).unwrap_or(&path).to_path_buf();
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let Some(rule) = rules
                .iter()
                .find(|r| r.includes.iter().any(|g| glob_match(g, &rel_str)))
            else {
                continue;
            };

            let stem = rel
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let bytes = compile(&path, &stem)?;
            let out_rel = burned_output_path(&rel, out_ext);
            let file = stored_file(&out_rel, bytes);
            let toc = get_or_create_toc(tocs, &rule.alias);
            insert_or_replace(&mut toc.root, &out_rel, file);
        }
    }
    Ok(())
}

/// Compiles reflection `.rdo` sources into `.bin`s (the `ReflectBurner`).
fn add_reflection_bins(tocs: &mut Vec<Toc>, assets: &Path) -> Result<()> {
    add_burned_files(
        tocs,
        assets,
        "ReflectBurner",
        "rdo",
        "bin",
        |path, _stem| {
            let rdo_xml = std::fs::read_to_string(path)?;
            relic_chunky::reflect_write::compile_bin(&rdo_xml)
                .map_err(|e| anyhow!("compiling {}: {e:#}", path.display()))
        },
    )
}

fn add_attribute_rgds(tocs: &mut Vec<Toc>, assets: &Path) -> Result<()> {
    add_burned_files(
        tocs,
        assets,
        "Mod Attributes",
        "xml",
        "rgd",
        |path, _stem| {
            let xml = std::fs::read_to_string(path)?;
            relic_chunky::attrib::compile_attrib(&xml)
                .map_err(|e| anyhow!("compiling {}: {e:#}", path.display()))
        },
    )
}

/// Compiles PNG sources into `.rrtex` textures (the `RRTextureBurner`).
fn add_texture_bins(tocs: &mut Vec<Toc>, assets: &Path) -> Result<()> {
    add_burned_files(
        tocs,
        assets,
        "RRTextureBurner",
        "png",
        "rrtex",
        |path, stem| {
            let png = std::fs::read(path)?;
            relic_chunky::texture::compile_texture(&png, stem)
                .map_err(|e| anyhow!("compiling {}: {e:#}", path.display()))
        },
    )
}

/// Compiles localization CSVs into `.ucs` string tables (the `UCS` burner).
/// The burnproj rule matches the `.locdb`; each sibling `<stem>_<locale>.csv`
/// becomes `<locale>/<locale>.ucs` in the rule's TOC.
fn add_localization(tocs: &mut Vec<Toc>, assets: &Path) -> Result<()> {
    let Some(burnproj) = find_burnproj(assets) else {
        return Ok(());
    };
    let rules = parse_burn_rules(&std::fs::read_to_string(&burnproj)?, "UCS");
    if rules.is_empty() {
        return Ok(());
    }

    let mut stack = vec![assets.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("locdb"))
            {
                continue;
            }
            let rel = path.strip_prefix(assets).unwrap_or(&path).to_path_buf();
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let Some(rule) = rules
                .iter()
                .find(|r| r.includes.iter().any(|g| glob_match(g, &rel_str)))
            else {
                continue;
            };

            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let prefix = format!("{stem}_");
            let locdb_dir = path.parent().unwrap_or(assets).to_path_buf();
            // Each sibling `<stem>_<locale>.csv` is one locale's string table.
            for sibling in std::fs::read_dir(&locdb_dir)?.flatten().map(|e| e.path()) {
                if !sibling
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
                {
                    continue;
                }
                let file_stem = sibling
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let Some(locale) = file_stem.strip_prefix(&prefix) else {
                    continue;
                };
                let ucs = crate::localization::compile_ucs(&std::fs::read(&sibling)?)
                    .map_err(|e| anyhow!("compiling {}: {e:#}", sibling.display()))?;
                let out_rel = PathBuf::from(locale).join(format!("{locale}.ucs"));
                let file = stored_file(&out_rel, ucs);
                let toc = get_or_create_toc(tocs, &rule.alias);
                insert_or_replace(&mut toc.root, &out_rel, file);
            }
        }
    }
    Ok(())
}

fn find_burnproj(assets: &Path) -> Option<PathBuf> {
    std::fs::read_dir(assets)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("burnproj"))
        })
}

/// Extracts the `ReflectBurner` rules from a `.burnproj` by lightweight tag
/// scanning (no XML dependency needed for its simple structure).
fn parse_burn_rules(xml: &str, burner: &str) -> Vec<BurnRule> {
    let mut rules = Vec::new();
    for block in xml.split("<BurnRule>").skip(1) {
        let block = block
            .split_once("</BurnRule>")
            .map(|(b, _)| b)
            .unwrap_or(block);
        if tag_value(block, "Burner").as_deref() != Some(burner) {
            continue;
        }
        let alias = tag_value(block, "Alias").unwrap_or_default();
        let includes: Vec<String> = tag_value(block, "Includes")
            .unwrap_or_default()
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if !alias.is_empty() && !includes.is_empty() {
            rules.push(BurnRule { alias, includes });
        }
    }
    rules
}

fn tag_value(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    Some(block[start..end].to_string())
}

/// The archive path for a burned source: same directory, lower-cased stem, and
/// the burner's output extension.
fn burned_output_path(rel: &Path, ext: &str) -> PathBuf {
    let stem = rel
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut out = rel.to_path_buf();
    out.set_file_name(format!("{stem}.{ext}"));
    out
}
fn glob_match(pattern: &str, path: &str) -> bool {
    let pat = pattern.replace('\\', "/");
    let pat_segs: Vec<&str> = pat.split('/').collect();
    let path_segs: Vec<&str> = path.split('/').collect();
    match_segments(&pat_segs, &path_segs)
}

fn match_segments(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| match_segments(rest, &path[i..])),
        Some((&head, rest)) => match path.split_first() {
            Some((seg, tail)) if seg_match(head.as_bytes(), seg.as_bytes()) => {
                match_segments(rest, tail)
            }
            _ => false,
        },
    }
}

/// Case-insensitive wildcard match within one path segment (`*` any run, `?`
/// one character).
fn seg_match(pat: &[u8], seg: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while si < seg.len() {
        if pi < pat.len() && (pat[pi] == b'?' || pat[pi].eq_ignore_ascii_case(&seg[si])) {
            pi += 1;
            si += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star = pi;
            mark = si;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

fn read_guid(dir: &Path) -> Result<String> {
    let aoe4mod = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("aoe4mod"))
        })
        .ok_or_else(|| anyhow!("no .aoe4mod file found in {}", dir.display()))?;
    let content = std::fs::read_to_string(&aoe4mod)?;
    let start = content
        .find("<ID>")
        .ok_or_else(|| anyhow!("no <ID> in {}", aoe4mod.display()))?
        + 4;
    let end = content[start..]
        .find("</ID>")
        .ok_or_else(|| anyhow!("malformed <ID> in {}", aoe4mod.display()))?
        + start;
    Ok(content[start..end].replace('-', ""))
}

fn folder_is_empty(folder: &Folder) -> bool {
    folder.files.is_empty() && folder.folders.is_empty()
}

/// The default file transform: pass bytes through unchanged.
fn identity_transform(_path: &Path, data: Vec<u8>) -> Result<Vec<u8>> {
    Ok(data)
}

fn add_source_files(
    root: &mut Folder,
    base: &Path,
    dir: &Path,
    only_ext: Option<&str>,
    storage: &FileStorageType,
    transform: &dyn Fn(&Path, Vec<u8>) -> Result<Vec<u8>>,
) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            add_source_files(root, base, &path, only_ext, storage, transform)?;
            continue;
        }

        if let Some(ext) = only_ext {
            let matches = path
                .extension()
                .map(|e| e.to_string_lossy().eq_ignore_ascii_case(ext))
                .unwrap_or(false);
            if !matches {
                continue;
            }
        }

        let rel = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let data = transform(&path, std::fs::read(&path)?)?;
        let stored = encode(&data, storage)?;
        let mut crc = Crc::new();
        crc.update(&stored);

        let file = FileEntry {
            name: file_name,
            stored_data: stored,
            uncompressed_size: data.len() as u32,
            storage_type: storage.clone(),
            encryption_type: FileEncryptionType::None,
            verification_type: FileVerificationType::SHA1Blocks,
            crc: crc.sum(),
            data_order: None,
        };
        insert_file(root, &rel, file);
    }

    Ok(())
}

fn build_from_dir(dir: &Path) -> Result<Folder> {
    let mut folders = Vec::new();
    let mut files = Vec::new();

    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            let mut sub = build_from_dir(&path)?;
            sub.name = file_name;
            folders.push(sub);
        } else {
            let data = std::fs::read(&path)?;
            let len = data.len() as u32;
            files.push(FileEntry {
                name: file_name,
                stored_data: data,
                uncompressed_size: len,
                storage_type: FileStorageType::Store,
                encryption_type: FileEncryptionType::None,
                verification_type: FileVerificationType::SHA1Blocks,
                crc: 0,
                data_order: None,
            });
        }
    }

    Ok(Folder {
        name: String::new(),
        folders,
        files,
    })
}
