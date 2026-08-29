# Domain vocabulary

Ubiquitous language for the sga-archiver workspace. Use these names in code,
commits, and design discussions.

## SGA archives (`crates/sga`)

- **Archive** — the parsed model of an `.sga` file: header metadata plus TOCs.
- **Main header** — the wire record at the start of an archive (magic, version,
  name, blob offsets, signature). One of the header's two wire records.
- **Index table** — the header's second wire record, at `header_blob_offset`:
  offsets and counts for the TOC/folder/file/string/hash tables.
- **TOC** — a table of contents; an archive has up to three (`data`, `info`,
  `locale`), and the game rejects mod files routed into the wrong one.
- **TOC layout generation** — how the TOC blob and data blob are ordered.
  `Legacy` (base game, older editor builds): depth-first, no string dedup.
  `Modern` (current editor): breadth-first with deduplicated strings.
  Detected on read; fresh packs are Modern.
- **Reserved header bytes** — version-specific regions the crate preserves
  verbatim rather than interpreting (`HeaderReserved`), so round-trips never
  rewrite bytes they didn't understand.
- **Build pipeline** — `compile_project`: source tree → burned files → routed
  TOCs → Archive, driven by the `.burnproj` rules.
- **Burner** — one source-to-asset compiler named by the burnproj:
  ReflectBurner (`.rdo`→`.bin`), RRTextureBurner (`.png`→`.rrtex`),
  Mod Attributes (`attrib\*.xml`→`.rgd`), UCS (`.csv`→`.ucs`).

## Relic Chunky formats (`crates/relic-chunky`)

- **Chunky** — the container format (`Relic Chunky\r\n\x1A\0`), recursive
  FOLD/DATA chunks.
- **Reflection file** — a `.bin` of RFTY/ROBJ/RFCI/RSHI/RNEW chunks; the
  compiled form of a `.rdo`.
- **`.rdo` dialect** — the XML source form of a reflection file (CRLF, tabs,
  fixed attribute order). DataValue lines carry the authoritative child order.
- **Serializer generation (reflection)** — mod-editor files defer out-of-line
  data in descending blob order; fully *reified* art files (strings/arrays as
  objects of their own) use ascending order.
- **RGD** — Relic Game Data: AEGD node tree + KEYS hash dictionary.

## Standing invariant

Byte-exact round-trips are the drift guard everywhere: read→write of any
supported file must reproduce its bytes, and the fresh pack of a mod source
must match the editor's own build.
