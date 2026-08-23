//! Read↔write drift guard for the Relic Chunky container: parsing a real file
//! and writing it back must reproduce the bytes exactly. If `read` and `write`
//! ever disagree on the layout, this fails.

use std::io::Cursor;

use relic_chunky::container::Chunky;

const MOD_BIN: &[u8] = include_bytes!("mod.bin");
const RGD: &[u8] = include_bytes!("weapon_war_elephant_spear_3_sul.rgd");

fn assert_round_trips(name: &str, bytes: &[u8]) {
    let chunky = Chunky::read(&mut Cursor::new(bytes)).unwrap();
    let mut out = Vec::new();
    chunky.write(&mut out).unwrap();
    assert_eq!(out, bytes, "{name}: Chunky read→write was not byte-exact");
}

#[test]
fn chunky_container_round_trips_byte_exact() {
    assert_round_trips("mod.bin (reflection)", MOD_BIN);
    assert_round_trips("weapon_war_elephant_spear_3_sul.rgd", RGD);
}
