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
Usage: sga-archiver --output <FILE> <INPUT>

Arguments:
  <INPUT>  Input file path

Options:
  -o, --output <FILE>          Output folder path
      --compile <ASSETS_DIR>   Recompile a mod's source into an archive instead of unpacking
  -h, --help                   Print help
  -V, --version                Print version
```

### Compile

With `--compile <ASSETS_DIR>`, the input is treated as a base archive (a previous build) and the output is a freshly packed archive. Source files found under the assets directory are recompressed into the archive; files with no source (burned artifacts such as textures, attributes, and localization) are carried over from the base build. Hashes are regenerated.

```
sga-archiver base.sga -o out.sga --compile ./assets
```

## Limitations
This has only been verified to work with AOE4 sga files, if you are experiencing any issues with other game sga files, just submit an issue, it shouldn't be too hard to implement it.

## Acknowledgement

Most of the code was translated from the C# project [`AOEMods.Essence`](https://github.com/aoemods/AOEMods.Essence). Even the documentation largely comes from there.
