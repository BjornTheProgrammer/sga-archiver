//! Minimal "does the mod load at all" build: takes the pristine house_pipeline
//! archive, strips every custom-art render-resource the game marks "not
//! permitted" (the knightsheep `.rrmaterial`/`.rrtex`/`.rrgeom`/`.rgm`/`.rgo`
//! and the hash-named `*_packed.rrtex`), leaves the scenario pointing at the
//! base-game house, and adds a mod descriptor + name. The result should pass
//! the game's mod-pack validation and appear in the list, isolating the
//! art-permission wall from everything else.
//!
//! Usage: cargo run -p sga --example minimal_mod_test -- <in.sga> <out.sga>

use std::env;
use std::fs::File;
use std::io::{BufReader, BufWriter};

use anyhow::Result;
use relic_chunky::reflect_write::compile_bin;
use sga::Archive;

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

fn ucs(entries: &[(u32, &str)]) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for (id, text) in entries {
        for u in format!("{id}\t{text}\r\n").encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
    out
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let (in_sga, out_sga) = (&args[1], &args[2]);

    let mut ar = Archive::read(&mut BufReader::new(File::open(in_sga)?))?;

    // Strip every knightsheep art file and the hash-named streaming packs. Leave
    // scenario minimap textures (`*_mm_generated.rrtex`) and everything else.
    let removed = ar.remove_files_where(|name| {
        name.contains("knightsheep") || name.ends_with("_packed.rrtex")
    });
    println!("stripped {removed} knightsheep art files");
    ar.prune_empty_folders();

    // Route by purpose into the right TOCs: descriptor -> info, loc -> locale.
    ar.upsert_stored_in("info", "mod.bin", compile_bin(MOD_RDO)?);
    ar.upsert_stored_in(
        "locale",
        "en/en.ucs",
        ucs(&[(1, "Knightsheep Test"), (2, "Load test — art stripped.")]),
    );

    ar.write(&mut BufWriter::new(File::create(out_sga)?))?;
    println!("wrote {out_sga}");
    Ok(())
}
