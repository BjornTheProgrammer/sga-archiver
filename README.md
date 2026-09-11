# SGA Archiver
A CLI tool to pack and unpack `.sga` archives from Relic.

## Installation
There are a few options of how to install sga-archiver, below are listed the ways.

### Cargo

`cargo install sga-archiver`

### Binary

Click the releases tab, and then download and install the version you wish to use.

## Usage

Two commands for people, and a set of JSON commands for other programs:

```
Usage: sga-archiver <COMMAND>

Commands:
  pack               Compile a mod source directory into an .sga archive
  unpack             Unpack an .sga archive into a directory
  inspect            Describe an archive and name its members, without reading their bytes (JSON)
  list               List members with size, storage, encryption and verification details (JSON)
  extract-member     Write one member's decoded bytes to a file (JSON)
  extract            Write selected members under a directory, keeping their archive paths (JSON)
  decode-rgd         Decode a compiled .rgd to XML (JSON)
  patch-rgd-cstring  Replace every CString equal to OLD with NEW inside an .rgd (JSON)
  patch-rgd-f32      Replace every Float under KEY equal to OLD with NEW inside an .rgd (JSON)
  compile-project    Compile a mod source directory and describe the result (JSON)
  repack             Read an archive and write it back (JSON)
  graft              Copy members from a donor archive into a base archive (JSON)
  graft-as           Copy donor members into a base archive under new paths (JSON)
  replace-member     Replace one existing member's bytes with a file's contents (JSON)
```

### Unpack

```
sga-archiver unpack <archive.sga> -o <out-dir>
```

Extracts the archive, writing a `.aoe4mod` project file and decompiling reflection files (`.bin` → `.txt` + `.rdo`) and game data (`.rgd` → `.xml`) alongside the raw files.

### Pack

```
sga-archiver pack <mod-source-dir> -o <out.sga>
```

The input is a mod source directory. It is expected to contain:

- `<mod>.aoe4mod` — the mod project file (its `<ID>` becomes the archive name)
- `<mod>.burnproj` (under `assets/`) — the editor project; its `ReflectBurner`, `RRTextureBurner`, and `UCS` rules tell the compiler which sources compile into which TOC
- `assets/scar/**/*.scar` — Lua source, compressed into the `data` TOC as-is
- `assets/**/*.rdo` — reflection source (win conditions, mod info, …) — **no compiled `.bin` needed**
- `assets/**/*.png` — texture source (mod preview image, UI art) — **no compiled `.rrtex` needed**
- `assets/locdb/<mod>_<locale>.csv` — localization source (`ID`/`Text` columns) — **no compiled `.ucs` needed**

Every artifact is **compiled directly from its source** — a mod tree needs no pre-burned binaries at all. The compiler reads the `.burnproj` rules to place each output in the right TOC (`mod.rdo` → `info`, `scar/winconditions/*.rdo` → `data`, `mod.png` → `info/mod.rrtex`, `<mod>_en.csv` → `locale/en/en.ucs`, …), lower-casing paths the way the editor does.

- **Reflection** (`.rdo` → `.bin`): the invariant engine type schema is bundled in the tool (keyed by root object type), so editing a `.rdo` and rebuilding changes win-condition options, labels, and other reflected data. When a `.rdo` is unchanged the output is byte-identical to the editor's.
- **Textures** (`.png` → `.rrtex`): single-mip BC7 (via `intel_tex_2`) in the Relic Chunky `TSET/TXTR/DXTC/TMAN/TDAT` container, zlib-segmented like the editor. Output is a functionally equivalent BC7 texture (not byte-identical — the editor's BC7/zlib encoders can't be reproduced exactly), verified to decode back to the source image near-losslessly.
- **Localization** (`.csv` → `.ucs`): UTF-16LE string table (`<id>\t<text>` per line) — byte-identical to the editor's.

Assets the tool can't yet compile (attributes `.rgd`, scenario formats) can still be supplied pre-burned under a `prebuilt/<toc>/<path>` directory and are packed as-is.

SHA1 hashes are generated and the archive is written unencrypted.

### Machine-readable commands

Every command other than `pack` and `unpack` prints exactly one JSON object on
stdout and nothing else. Each result carries `schema` (for example
`sga-archiver.list/v1`) and `tool` (`{"name", "version"}`). On failure nothing
is written to stdout, a message goes to stderr, and the exit code is nonzero.

`inspect`, `list`, `extract-member` and `extract` read lazily: they parse the
header and tables and touch only the member bytes they need, so they are the
right way to look inside a multi-gigabyte base-game archive. `list --verify`
additionally reads every member once and checks its stored bytes against the
recorded CRC32 and SHA1 block hashes; it does not validate the header
signature.

```
sga-archiver list Data.sga --filter .rrmaterial
sga-archiver extract-member Data.sga art/house.rgm out/house.rgm
sga-archiver extract Data.sga slot/ --filter .rrmaterial
sga-archiver graft base.sga donor.sga out.sga art/custom.rgm
```

Members are named with `/` in results and accepted with either separator on
the command line. Exact matches win; otherwise paths compare case-insensitively.

```
sga-archiver pack "./My Mod" -o out.sga
```

If a mod uses a reflection root type the tool doesn't yet bundle a schema for, packing fails with `no bundled schema for root type '<Type>'`. Regenerate the schema resource from an existing `.bin` of that type with `relic_chunky::reflect_write::extract_schema`, drop the `<RootType>.schema` file into `crates/relic-chunky/schemas/`, and add a match arm in `schema_lib.rs`.

## Limitations
This has only been verified to work with AOE4 sga files, if you are experiencing any issues with other game sga files, just submit an issue, it shouldn't be too hard to implement it.

## Acknowledgement

Most of the code was translated from the C# project [`AOEMods.Essence`](https://github.com/aoemods/AOEMods.Essence). Even the documentation largely comes from there.
