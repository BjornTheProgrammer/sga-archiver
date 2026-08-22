# SGA Archiver
A CLI tool to pack and unpack `.sga` archives from Relic.

## Installation
There are a few options of how to install sga-archiver, below are listed the ways.

### Cargo

`cargo install sga-archiver`

### Binary

Click the releases tab, and then download and install the version you wish to use.

## Usage

Two commands:

```
Usage: sga-archiver <COMMAND>

Commands:
  pack    Compile a mod source directory into an .sga archive
  unpack  Unpack an .sga archive into a directory
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
- `<mod>.burnproj` (under `assets/`) — the editor project; its `ReflectBurner` and `RRTextureBurner` rules tell the compiler which sources compile into which TOC
- `assets/scar/**/*.scar` — Lua source, compressed into the `data` TOC as-is
- `assets/**/*.rdo` — reflection source (win conditions, mod info, …), the editable source of truth — **no compiled `.bin` needed**
- `assets/**/*.png` — texture source (mod preview image, UI art) — **no compiled `.rrtex` needed**
- `prebuilt/<toc>/<path>` — pre-burned binary assets that genuinely can't be produced from text source (localization `.ucs`), grouped by TOC alias, e.g. `prebuilt/locale/en/en.ucs`

Reflection and texture artifacts are **compiled directly from their sources** — the mod tree carries no `.bin` or `.rrtex`. The compiler reads the `.burnproj` rules to place each output in the right TOC (`mod.rdo` → `info`, `scar/winconditions/*.rdo` → `data`, `mod.png` → `info/mod.rrtex`, …), lower-casing paths the way the editor does.

- **Reflection** (`.rdo` → `.bin`): the invariant engine type schema is bundled in the tool (keyed by root object type), so editing a `.rdo` and rebuilding changes win-condition options, labels, and other reflected data. When a `.rdo` is unchanged the output is byte-identical to the editor's.
- **Textures** (`.png` → `.rrtex`): single-mip BC7 (via `intel_tex_2`) in the Relic Chunky `TSET/TXTR/DXTC/TMAN/TDAT` container, zlib-segmented like the editor. Output is a functionally equivalent BC7 texture (not byte-identical — the editor's BC7/zlib encoders can't be reproduced exactly), verified to decode back to the source image near-losslessly.

SHA1 hashes are generated and the archive is written unencrypted.

```
sga-archiver pack "./My Mod" -o out.sga
```

If a mod uses a reflection root type the tool doesn't yet bundle a schema for, packing fails with `no bundled schema for root type '<Type>'`. Regenerate the schema resource from an existing `.bin` of that type with `relic_chunky::reflect_write::extract_schema`, drop the `<RootType>.schema` file into `crates/relic-chunky/schemas/`, and add a match arm in `schema_lib.rs`.

## Limitations
This has only been verified to work with AOE4 sga files, if you are experiencing any issues with other game sga files, just submit an issue, it shouldn't be too hard to implement it.

## Acknowledgement

Most of the code was translated from the C# project [`AOEMods.Essence`](https://github.com/aoemods/AOEMods.Essence). Even the documentation largely comes from there.
