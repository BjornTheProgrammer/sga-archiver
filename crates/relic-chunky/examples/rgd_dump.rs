use std::io::BufReader;
fn main(){let p=std::env::args().nth(1).unwrap();let mut cf=relic_chunky::chunky::ChunkFile::parse(BufReader::new(std::fs::File::open(&p).unwrap())).unwrap();let nodes=relic_chunky::rgd::RelicGameData::parse(&mut cf).unwrap();print!("{}",relic_chunky::rgd::game_data_to_xml(&nodes).unwrap());}
