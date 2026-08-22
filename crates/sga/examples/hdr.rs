fn main(){let p=std::env::args().nth(1).unwrap();let h=sga::read_header(&p).unwrap();
 println!("hdr_off={} hdr_len={} data_off={} data_len={} toc_off={} toc_cnt={} fold_off={} fold_cnt={} file_off={} file_cnt={} str_off={} str_len={} hash_off={} hash_len={}",
 h.header_blob_offset,h.header_blob_length,h.data_offset,h.data_blob_length,h.toc_data_offset,h.toc_data_count,h.folder_data_offset,h.folder_data_count,h.file_data_offset,h.file_data_count,h.string_offset,h.string_length,h.file_hash_offset,h.file_hash_length);}
