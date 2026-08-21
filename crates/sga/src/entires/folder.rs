use binrw::binrw;

use crate::utils::{parse_index, write_index};

#[binrw]
#[brw(little, import(version: u16))]
#[derive(Debug, Clone)]
pub struct SgaFolderEntry {
    pub name_offset: u32,

    #[br(parse_with = parse_index, args(version))]
    #[bw(write_with = write_index, args(version))]
    pub folder_start_index: u32,

    #[br(parse_with = parse_index, args(version))]
    #[bw(write_with = write_index, args(version))]
    pub folder_end_index: u32,

    #[br(parse_with = parse_index, args(version))]
    #[bw(write_with = write_index, args(version))]
    pub file_start_index: u32,

    #[br(parse_with = parse_index, args(version))]
    #[bw(write_with = write_index, args(version))]
    pub file_end_index: u32,
}
