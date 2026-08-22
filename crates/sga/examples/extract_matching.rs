//! Extract files whose archive path contains a substring.
//! Usage: extract_matching <archive.sga> <substr> <outdir>
use std::path::Path;
fn walk(folder:&sga::Folder, prefix:&str, sub:&str, out:&Path, n:&mut usize){
    for f in &folder.files {
        let p = if prefix.is_empty(){f.name.clone()}else{format!("{prefix}/{}",f.name)};
        if p.to_lowercase().contains(sub){
            if let Ok(data)=f.decoded(){ let dst=out.join(&p); std::fs::create_dir_all(dst.parent().unwrap()).ok(); std::fs::write(&dst,&data).ok(); *n+=1; if *n<=40 {println!("{p} ({} bytes)",data.len());} }
        }
    }
    for sf in &folder.folders { let p=if prefix.is_empty(){sf.name.clone()}else{format!("{prefix}/{}",sf.name)}; walk(sf,&p,sub,out,n); }
}
fn main(){
    let a:Vec<String>=std::env::args().collect();
    let arch=sga::read_archive(&a[1]).unwrap();
    let out=Path::new(&a[3]); let mut n=0;
    for toc in &arch.tocs { walk(&toc.root,"",&a[2].to_lowercase(),out,&mut n); }
    println!("== {n} files matched ==");
}
