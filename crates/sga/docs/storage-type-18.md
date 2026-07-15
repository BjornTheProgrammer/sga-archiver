# Storage type 18: `BufferCompress` + AES-128

`FileStorageType` byte values `0`-`4` are documented (`Store`, `StreamCompress`,
`BufferCompress`, `StreamCompressBrotli`, `BufferCompressBrotli`), and both this
crate and the reference C# implementation ([`AOEMods.Essence`][essence]) only
ever handle those five. In `Attrib.sga`/`Data.sga`-style archives shipped by
Age of Empires IV (archive format version 10), a handful of files carry the
value `18` (`0x12`) instead. Neither implementation recognized it before this
was investigated; both silently treated it as "unknown" and copied the bytes
verbatim, producing garbage output with no error.

This document records what was determined about it, in depth, including the
parts that remain unresolved.

## The byte is packed, not a new enum value

`18` decodes as two 4-bit fields packed into one byte:

```text
0x12 = 0001 0010
       ^^^^ ^^^^
       |    low nibble  (0x2) -> FileStorageType::BufferCompress
       high nibble (0x1) -> FileEncryptionType::AES128
```

i.e. `storage_byte = (encryption_type << 4) | storage_type`. For every file
without encryption, the high nibble is `0`, so the byte is numerically
identical to the plain `FileStorageType` value — which is exactly why this
went unnoticed: it's indistinguishable from the existing scheme until a file
actually sets the encryption nibble.

This was confirmed directly against Relic's own code (see below), not
inferred from the byte value alone.

## What's actually happening: zlib, then AES-128

The payload is: take the plaintext, deflate/zlib-compress it (identical to
plain `BufferCompress` — same 2-byte-header-then-raw-deflate scheme this
crate already implements for storage type `2`), then encrypt the compressed
blob with AES-128. There is no new compression algorithm here; if you had the
key, decrypting first and running the existing `BufferCompress` path
unmodified would recover the original file.

## Evidence trail

### 1. Corpus-wide scan

Across all 138 `.sga` archives shipped with the game (331,000+ files total),
value `18` appears on exactly **13 files**, all inside `Data.sga`, all under
`scar\terrainlayout\{skirmish_maps,test_maps}\`. No other storage-type byte
outside `0`-`4` appears anywhere else in the corpus. The 13 files:

```
scar\terrainlayout\skirmish_maps\acropolis.lua
scar\terrainlayout\skirmish_maps\african_waters.lua
scar\terrainlayout\skirmish_maps\cliffside.lua
scar\terrainlayout\skirmish_maps\golden_pit.lua
scar\terrainlayout\skirmish_maps\gorge.lua
scar\terrainlayout\skirmish_maps\haunted_gulch.lua
scar\terrainlayout\skirmish_maps\highland.lua
scar\terrainlayout\skirmish_maps\migration.lua
scar\terrainlayout\skirmish_maps\mystical_river.lua
scar\terrainlayout\skirmish_maps\sacred_crest.lua
scar\terrainlayout\skirmish_maps\volcanic_island.lua
scar\terrainlayout\skirmish_maps\wadden_sea.lua
scar\terrainlayout\test_maps\00_vivid_acropolis.lua
```

178 other `.lua` files sit in the same two directories (`combat_test.lua`,
`mesh_test.lua`, `generated_campaign_map.lua`, etc.) and are plain
`BufferCompress` (`2`), decoding to readable Lua normally. The split is not
obviously "important vs. unimportant" — the maps in this list are not
confirmed to be the current ranked pool (see "Open questions" below), and no
map named here carries an internal `3.0`/version marker distinguishing it
from the unencrypted set.

### 2. Raw bytes rule out anything but real encryption

For each of the 13 files:

- Shannon entropy is 7.88-7.99 bits/byte (max possible is 8) — consistent
  with either compressed or encrypted data, not with plaintext.
- No two files share a byte in common at the same offset, and none match the
  magic bytes of gzip, zlib, zstd, lz4 (frame), bzip2, xz, 7z, zip, lzo, or
  framed snappy — ruling out a headerless variant of any mainstream open
  codec being silently mis-tagged.
- `compressed_length` is consistently ~5-6x smaller than `uncompressed_size`
  (e.g. 1885 -> 11298 bytes). Pure encryption does not shrink data — if these
  were encrypted-only (no compression underneath), ciphertext size would
  track plaintext size 1:1 (plus fixed overhead). The real size reduction is
  what pins this down as "compress, then encrypt" rather than "encrypt only".

None of `zstd::decode_all`, `lz4_flex::decompress_size_prepended`, or
`bzip2::read::BzDecoder` succeed on the raw bytes either (checked
programmatically, not just by magic-byte inspection).

### 3. Confirmed directly via Relic's own code (managed)

`Essence.Core.dll`, which ships in the game's install directory
(`Essence.Core.dll` next to `RelicCardinal.exe`), is a real .NET assembly and
is the actual library backing `AOEMods.Essence`-style tooling. Loading it via
reflection (`[System.Reflection.Assembly]::LoadFrom(...)` from PowerShell —
no decompiler needed) exposes:

```
Essence.Core.IO.Archive.FileStorageType    { Store=0, StreamCompress=1, BufferCompress=2, StreamCompressBrotli=3, BufferCompressBrotli=4 }
Essence.Core.IO.Archive.FileEncryptionType { None=0, AES128=1 }
Essence.Core.IO.Archive.File.StorageType   -> FileStorageType   (separate property)
Essence.Core.IO.Archive.File.EncryptionType -> FileEncryptionType (separate property)
```

Opening `Data.sga` and resolving `acropolis.lua` through this library reports:

```
StorageType=BufferCompress EncryptionType=AES128
```

confirming the nibble-split decoding above from Relic's own type system, not
just from byte arithmetic.

Calling `File.GetData()` (which internally calls
`Archive.OpenRead(File)`) throws:

```
System.ApplicationException: Key not set.
   at Essence.Core.IO.Archive.Archive.OpenRead(File file)
```

`Archive.Key` is a public settable `byte[]` property, and there's a
`KeyResolver` delegate type (`Essence.Core.DictionaryKey Invoke(UInt64)`) used
to resolve keys dynamically. `DictionaryKey` is a name/hash pair
(`{ Hash: ulong, String: string }`), **not** the raw key bytes — it's an
identifier, not key material. Nothing in `Essence.Core.dll` supplies a
default/fallback key; it is unconditionally required from the caller.

### 4. The official mod editor also doesn't ship the key

`EssenceEditor.exe` (Relic's own archive/content editor, also present in the
install directory) has full UI support for encrypted-archive awareness:
`Essence.Editor.ArchiveEditor.Converters.CanDecryptConverter` and
`ArchiveControl.CanDecrypt(ArchiveProxyFile)`. Disassembling `CanDecrypt`'s IL
directly shows it does nothing but check whether `Archive.Key` is already
non-null — it doesn't supply, derive, or embed a key anywhere. Scanning every
embedded resource across all 13 managed assemblies shipped with the game
(editor themes, syntax-highlighting definitions, completion databases, etc.)
turned up nothing key-shaped either.

Conclusion: **even Relic's own public editor tooling does not carry this
key.** Whatever supplies it lives entirely outside anything distributed to
players or modders.

### 5. Native confirmation, in `RelicCardinal.exe`

The managed-code findings were independently corroborated by inspecting the
main native game binary directly (PE parsing + x86-64 disassembly, not a
decompiler):

- **Import table**: `RelicCardinal.exe` imports `CryptDecrypt` from
  `ADVAPI32.dll`, but *no* `CryptImportKey`/`BCryptImportKey` for symmetric
  keys. The `CRYPT32`/`bcrypt` imports present (`CertGetCertificateContextProperty`,
  `CertVerifyCertificateChainPolicy`, `BCryptSignHash`, `BCryptGenerateKeyPair`,
  etc.) are all certificate/signature-verification APIs, unrelated to this.
  This rules out the AES-128 operation going through Windows CryptoAPI/CNG at
  all.
- **AES S-box signature**: the standard 256-byte Rijndael forward S-box
  appears in `.rdata` **exactly once** in the whole 144 MB binary, with the
  inverse S-box immediately following it (the layout a from-scratch software
  AES implementation uses) — confirming a self-contained, statically linked
  AES implementation, not an OS API call.
- **The key-schedule routine**: found by locating the two code sites that
  reference the S-box (both `lea rbx, [rip+<sbox address>]`). Disassembly of
  the surrounding function shows a textbook AES-128 key expansion:
  ```asm
  mov  r9d, 10h              ; key size = 16 bytes -> AES-128 path
  cmp  r9, 10h
  jne  <AES-192/256 path elsewhere>
  movups xmm0, [r13+0F0h]    ; raw 16-byte key read from offset +0xF0
                              ; of the object passed in as the 1st argument (rcx -> r13)
  ...                        ; xor/shr/imul-by-0x1B loop = standard Rijndael
                              ; round-key expansion (0x1B is the AES GF(2^8)
                              ; reduction polynomial constant)
  ```
  This is generic, key-agnostic machinery — it expands whatever 16 bytes are
  already sitting at a fixed offset in the caller's object. There is no
  literal/hardcoded key value in this function or its surrounding data.
- **Tracing the caller**: the key-schedule function has exactly one direct
  caller. That caller passes `rcx = <some object> + 0x20` as the 1st
  argument (so the raw key ultimately lives at a fixed offset,
  object+0x20+0xF0, inside a larger structure). Tracing one level further
  up, that function itself has exactly one caller (a thin pass-through
  wrapper), which in turn has **three** callers clustered together in the
  binary, alongside what looks like a small family of related methods
  (matching constructor-like patterns touching nearby offsets `+0x148`,
  `+0x150`, `+0x160` on the same object type).
- A separate lookup/resolution call found nearby (initially suspected to be
  a "key dictionary" lookup, matching the managed `KeyResolver` delegate
  concept) turned out on closer inspection to be an unrelated internal
  mechanism — a lazy, hash-based PE-export-table symbol resolver (recognizable
  from the FNV-1a hash constants `0x811C9DC5`/`0x1000193` and PE
  `e_lfanew`/export-directory offset arithmetic in its disassembly) used
  elsewhere in the same function, not the source of the raw key bytes.

**This is where the investigation stopped.** The next step in the trace
would require constructing the object type at that `+0x20`/`+0xF0` location
(effectively full call-graph/type recovery over a stripped, optimized,
144 MB binary) or dynamic analysis (live debugging, memory inspection at the
point of use, or binary patching) to observe the key value directly.
Live/dynamic extraction of a key that Relic has gone out of its way to keep
out of every shipped artifact was judged to be a different category of
activity than static file-format documentation — defeating a deliberate
access control rather than describing one — and was not pursued.

## Current behavior of this crate

[`FileStorageType`][file-rs] has an explicit `Aes128Encrypted` variant:
[`FileStorageType::from_u8`][file-rs] checks the byte's high nibble and, when
it's `1`, returns `Aes128Encrypted` directly (byte `18` -> `Aes128Encrypted`).
The low nibble (the wrapped compression scheme) is deliberately *not*
retained — without the key it can never actually be exercised, so there's
nothing to gain from carrying it around. A high nibble of anything other
than `0` or `1` falls back to `Unknown(n)`, as does a low nibble outside
`0`-`4` when the high nibble is `0`.

[`FileNode::read_data`][file-node-rs] recognizes `Aes128Encrypted` explicitly
and, since the key isn't available, copies the stored bytes verbatim (the
same as `Store`/`Unknown`) rather than attempting decompression — the
returned bytes are still ciphertext. `sga-unpacker` reports this distinctly
at extraction time (`the storage type of '<path>' is AES-128 encrypted,
which this crate can't decrypt, ...`, with a link to this document) rather
than lumping it in with the generic "unknown storage type" warning, which is
now reserved for byte values that genuinely aren't understood at all.

## Open questions

- **The actual key value.** Not recovered, and (per the above) not pursued
  further once the investigation reached the point of needing dynamic
  analysis or binary patching to continue.
- **Why these specific 13 files.** The map names are genuine skirmish/map
  names, but they are not confirmed to be the current ranked ladder pool —
  if anything, the *unencrypted* neighboring files are more likely to be the
  active ranked maps, which undercuts a simple "protect competitive
  integrity" explanation. Equally plausible: retired/legacy maps, an
  artifact of whichever files happened to pass through a particular
  internal build step, or something about these specific files unrelated to
  their in-game importance. Nothing in the archive itself indicates which.
- **Whether `16`/`17`/`19`/`20` (the other AES-flagged combinations) are used
  anywhere.** Not observed in this corpus; `18` (`AES128 | BufferCompress`)
  is the only combination seen across all 138 archives.
- **Whether other Relic titles sharing this archive format/engine lineage**
  (e.g. Company of Heroes 3) use this same encryption scheme. Not
  investigated here.

[essence]: https://github.com/aoemods/AOEMods.Essence
[file-rs]: ../src/entires/file.rs
[file-node-rs]: ../src/nodes/file.rs
