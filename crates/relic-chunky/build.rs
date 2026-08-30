fn main() {
    // intel_tex_2 links prebuilt C++/ISPC static libraries but does not link
    // the C++ runtime they need (`__gxx_personality_v0`). MSVC pulls its
    // runtime in implicitly; Linux and macOS must be told.
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => println!("cargo:rustc-link-lib=dylib=stdc++"),
        Ok("macos") => println!("cargo:rustc-link-lib=dylib=c++"),
        _ => {}
    }
}
