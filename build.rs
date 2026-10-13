// Builds and links kiln (the C runtime over llama.cpp).
//
//   default            build third_party/kiln (a git submodule) with CMake into OUT_DIR
//   KILN_DIR=<path>    build that kiln checkout instead of the submodule
//   KILN_LIB_DIR=<dir> skip building; link the libkiln.so that is in <dir>
//
// The built library is copied next to the binaries (target/<profile>/), and the binaries
// look for it in $ORIGIN, $ORIGIN/../lib and $ORIGIN/../lib/infind, so a plain
// `cargo build` runs from the target folder and an installed or unpacked copy runs from
// wherever it is put.
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-env-changed=KILN_DIR");
    println!("cargo:rerun-if-env-changed=KILN_LIB_DIR");
    println!("cargo:rerun-if-env-changed=KILN_CMAKE_ARGS");
    println!("cargo:rerun-if-changed=build.rs");

    let lib_dir = match std::env::var("KILN_LIB_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => {
            let src = std::env::var("KILN_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| manifest.join("third_party").join("kiln"));
            build_kiln(&src, &out)
        }
    };
    let lib_dir = lib_dir.canonicalize().unwrap_or(lib_dir);

    // Put the library beside the binaries: target/<profile>/ is three levels above OUT_DIR.
    let profile_dir = out.ancestors().nth(3).map(Path::to_path_buf).unwrap_or_else(|| lib_dir.clone());
    if profile_dir != lib_dir {
        if let Ok(rd) = std::fs::read_dir(&lib_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("libkiln.so") {
                    let _ = std::fs::copy(e.path(), profile_dir.join(&name));
                }
            }
        }
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=kiln");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib/infind");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
}

fn build_kiln(src: &Path, out: &Path) -> PathBuf {
    if !src.join("CMakeLists.txt").exists() {
        panic!(
            "kiln sources not found at {}.\n\
             Fetch them with:  git submodule update --init --recursive\n\
             or point KILN_DIR at a kiln checkout, or KILN_LIB_DIR at a built libkiln.so.",
            src.display()
        );
    }
    println!("cargo:rerun-if-changed={}", src.join("CMakeLists.txt").display());
    println!("cargo:rerun-if-changed={}", src.join("src").display());
    println!("cargo:rerun-if-changed={}", src.join("include").display());

    // kiln fetches llama.cpp itself when its folder is empty, but only if the checkout is a
    // git repository; a plain source archive has to ship it.
    let llama = src.join("third_party").join("llama.cpp");
    if !llama.join("CMakeLists.txt").exists() && src.join(".git").exists() {
        let _ = Command::new("git")
            .args(["submodule", "update", "--init", "--recursive"])
            .current_dir(src)
            .status();
    }

    let build = out.join("kiln-build");
    let mut cfg = Command::new("cmake");
    cfg.arg("-S").arg(src).arg("-B").arg(&build);
    cfg.args(["-DCMAKE_BUILD_TYPE=Release", "-DKILN_BUILD_CLI=OFF", "-DKILN_BUILD_TESTS=OFF"]);
    if let Ok(extra) = std::env::var("KILN_CMAKE_ARGS") {
        cfg.args(extra.split_whitespace());
    }
    run(&mut cfg, "configure kiln (is cmake installed?)");

    let jobs = std::env::var("NUM_JOBS").unwrap_or_else(|_| "4".into());
    let mut b = Command::new("cmake");
    b.arg("--build").arg(&build).args(["--target", "kiln", "--config", "Release", "-j", &jobs]);
    run(&mut b, "build kiln");

    for cand in [build.clone(), build.join("Release")] {
        if cand.join("libkiln.so").exists() {
            return cand;
        }
    }
    panic!("kiln built, but libkiln.so was not found under {}", build.display());
}

fn run(cmd: &mut Command, what: &str) {
    let st = cmd.status().unwrap_or_else(|e| panic!("could not {what}: {e}"));
    if !st.success() {
        panic!("failed to {what}");
    }
}
