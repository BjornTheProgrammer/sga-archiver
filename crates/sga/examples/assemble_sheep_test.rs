//! De-risk step for getting the knightsheep model in-game: assemble a mod
//! archive whose scenario spawns a knightsheep object built from KNOWN-GOOD
//! geometry (the dev's already-compiled `eng_house_age2`) wearing the sheep's
//! real material. If this renders in-game (house shape, sheep skin), the whole
//! pipeline — custom-path packaging, `.rgm` base-path binding, scenario spawn,
//! repack — is proven, and only the vertex buffers remain to swap.
//!
//! Usage: cargo run -p sga --example assemble_sheep_test -- <in.sga> <house_dir> <out.sga>

use std::env;
use std::fs::File;
use std::io::{BufReader, BufWriter};

use anyhow::{Context, Result};
use relic_chunky::container::replace_length_prefixed_strings;
use relic_chunky::reflect_write::compile_bin;
use sga::Archive;

// Mod descriptor for GUID 2ec8e8c5-bf88-47a3-a1ee-9793a9580ecf. The four
// m_modPart values are that GUID re-encoded: Data1; (Data3<<16)|Data2; then two
// little-endian u32s from the trailing 8 bytes. The name/description resolve via
// loc keys 1/2 in `en/en.ucs`.
const MOD_RDO: &str = r#"<DataWarehouse>
<!--Mod/-->
<DataObject Name="" Type="Mod" Id="3268000000000000001">
<DataProperty Name="m_name" Type="Object">
<DataValue Name="util::ReflectLocString">3268000000000000002</DataValue>
<DataObject Name="" Type="util::ReflectLocString" Id="3268000000000000002" OwnerId="3268000000000000001">
<DataProperty Name="m_locStringKey" Type="Int32" Value="1"/>
<DataProperty Name="m_modPart0" Type="UInt32" Value="786596549"/>
<DataProperty Name="m_modPart1" Type="UInt32" Value="1201439112"/>
<DataProperty Name="m_modPart2" Type="UInt32" Value="2476514977"/>
<DataProperty Name="m_modPart3" Type="UInt32" Value="3473943721"/>
</DataObject>
</DataProperty>
<DataProperty Name="m_description" Type="Object">
<DataValue Name="util::ReflectLocString">3268000000000000003</DataValue>
<DataObject Name="" Type="util::ReflectLocString" Id="3268000000000000003" OwnerId="3268000000000000001">
<DataProperty Name="m_locStringKey" Type="Int32" Value="2"/>
<DataProperty Name="m_modPart0" Type="UInt32" Value="786596549"/>
<DataProperty Name="m_modPart1" Type="UInt32" Value="1201439112"/>
<DataProperty Name="m_modPart2" Type="UInt32" Value="2476514977"/>
<DataProperty Name="m_modPart3" Type="UInt32" Value="3473943721"/>
</DataObject>
</DataProperty>
</DataObject>
</DataWarehouse>"#;

/// Builds a `.ucs` (UTF-16LE + BOM, `<id>\t<text>\r\n` per line).
fn ucs(entries: &[(u32, &str)]) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for (id, text) in entries {
        for u in format!("{id}\t{text}\r\n").encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
    out
}

const OLD_BASE: &str = r"art\scenario\eng_house_age2\eng_house_age2";
const NEW_BASE: &str = r"art\scenario\a4etk\knightsheep\knightsheep_m00_a52ae927";
const OLD_RGO: &str = r"generic:art\scenario\eng_house_age2\eng_house_age2.rgo";
const NEW_RGO: &str = r"generic:art\scenario\a4etk\knightsheep\knightsheep_m00_a52ae927.rgo";
const KS_DIR: &str = "art/scenario/a4etk/knightsheep";
const LAYER: &str =
    "scenarios/multiplayer/house_pipeline_test/house_pipeline_test/imported houses.layer";

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let (in_sga, house_dir, out_sga) = (&args[1], &args[2], &args[3]);

    let mut ar = Archive::read(&mut BufReader::new(File::open(in_sga)?))?;

    // The a4etk material burn emitted streaming `*_packed.rrtex` texture packs,
    // which the game rejects in a mod ("… not permitted" → invalid file
    // structure → the whole mod is dropped). Strip them so the mod loads.
    let stripped = ar.remove_files_where(|name| name.ends_with("_packed.rrtex"));
    println!("stripped {stripped} forbidden *_packed.rrtex files");

    println!(
        "loaded {in_sga}: v{} tocs=[{}]",
        ar.version,
        ar.tocs.iter().map(|t| t.alias.as_str()).collect::<Vec<_>>().join(", ")
    );

    // Known-good compiled geometry + object from the dev's template house.
    let rgm = std::fs::read(format!("{house_dir}/eng_house_age2.rgm"))?;
    let rrgeom = std::fs::read(format!("{house_dir}/eng_house_age2.rrgeom"))?;
    let rgo = std::fs::read(format!("{house_dir}/eng_house_age2.rgo"))?;

    // Retarget the .rgm's base path so it binds to the knightsheep geometry +
    // material instead of the house's.
    let (rgm, n) = replace_length_prefixed_strings(&rgm, &[(OLD_BASE, NEW_BASE)])?;
    println!("patched .rgm base-path tokens: {n}");
    anyhow::ensure!(n >= 1, "expected to retarget the .rgm base path at least once");

    ar.upsert_stored(&format!("{KS_DIR}/knightsheep_m00_a52ae927.rgm"), rgm);
    ar.upsert_stored(&format!("{KS_DIR}/knightsheep_m00_a52ae927.rrgeom"), rrgeom);
    ar.upsert_stored(&format!("{KS_DIR}/knightsheep_m00_a52ae927.rgo"), rgo);

    // Repoint the scenario layer from the house object to the knightsheep object.
    let layer = ar.read_file(LAYER).context("layer not found in archive")?;
    let (layer, m) =
        replace_length_prefixed_strings(&layer, &[(OLD_RGO, NEW_RGO), (OLD_BASE, NEW_BASE)])?;
    println!("patched .layer object-ref tokens: {m}");
    anyhow::ensure!(m >= 1, "expected to repoint the scenario layer at least once");
    ar.upsert_stored(LAYER, layer);

    // Mod descriptor + localization so the game lists it as a playable mod.
    let mod_bin = compile_bin(MOD_RDO)?;
    println!("compiled mod.bin: {} bytes", mod_bin.len());
    ar.upsert_stored("mod.bin", mod_bin);
    ar.upsert_stored("mod.rdo", MOD_RDO.as_bytes().to_vec());
    ar.upsert_stored(
        "en/en.ucs",
        ucs(&[(1, "Knightsheep Test"), (2, "Custom model render de-risk test.")]),
    );

    ar.write(&mut BufWriter::new(File::create(out_sga)?))?;
    println!("wrote {out_sga}");
    Ok(())
}
