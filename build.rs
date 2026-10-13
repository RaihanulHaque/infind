// Links infind against kiln's shared library (libkiln.so), built with
// `cmake --preset host && cmake --build --preset host` inside the kiln repo.
// KILN_DIR points at the kiln checkout; default is ../kiln next to this crate.
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let kiln = std::env::var("KILN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("..").join("kiln"));
    let lib = std::env::var("KILN_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| kiln.join("build").join("host"));
    let lib = lib.canonicalize().unwrap_or(lib);
    if !lib.join("libkiln.so").exists() {
        panic!(
            "libkiln.so not found in {}. Build kiln first:\n  cd {} && cmake --preset host && cmake --build --preset host -j\nor set KILN_DIR / KILN_LIB_DIR.",
            lib.display(),
            kiln.display()
        );
    }
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=dylib=kiln");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
    println!("cargo:rerun-if-env-changed=KILN_DIR");
    println!("cargo:rerun-if-env-changed=KILN_LIB_DIR");
    println!("cargo:rerun-if-changed=build.rs");
}
