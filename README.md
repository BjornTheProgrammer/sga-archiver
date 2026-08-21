# SGA Archiver
A CLI tool to pack and unpack `.sga` archives from Relic.

## Installation
There are a few options of how to install sga-archiver, below are listed the ways.

### Cargo

`cargo install sga-archiver`

### Binary

Click the releases tab, and then download and install the version you wish to use.

## Usage

### Unpack

Run sga-archiver with an input archive and an output directory. It will be unpacked.

```
Usage: sga-archiver [OPTIONS] --output <FILE> <INPUT>

Arguments:
  <INPUT>  Input archive to unpack, or mod source directory when --compile is set

Options:
  -o, --output <FILE>  Output folder (unpack) or output archive (compile)
      --compile        Compile a mod source directory into an archive
  -h, --help           Print help
  -V, --version        Print version
```

### Compile

With `--compile`, the input is a mod source directory and the output is a packed `.sga`. The directory is expected to contain:

- `<mod>.aoe4mod` — the mod project file (its `<ID>` becomes the archive name)
- `assets/scar/**/*.scar` — Lua source, compressed into the `data` TOC as-is
- `prebuilt/<toc>/<path>` — pre-burned artifacts that can't be produced from source (textures, attributes, localization), grouped by TOC alias, e.g. `prebuilt/info/mod.rrtex`, `prebuilt/locale/en/en.ucs`, `prebuilt/data/scar/winconditions/<name>.bin`

SHA1 hashes are generated and the archive is written unencrypted.

```
sga-archiver "./My Mod" -o out.sga --compile
```

## Limitations
This has only been verified to work with AOE4 sga files, if you are experiencing any issues with other game sga files, just submit an issue, it shouldn't be too hard to implement it.

## Acknowledgement

Most of the code was translated from the C# project [`AOEMods.Essence`](https://github.com/aoemods/AOEMods.Essence). Even the documentation largely comes from there.
