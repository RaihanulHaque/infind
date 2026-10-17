// Builds and links kiln (the C runtime over llama.cpp).
//
//   default            build third_party/kiln (a git submodule) with CMake into OUT_DIR
//   KILN_DIR=<path>    build that kiln checkout instead of the submodule
//   KILN_LIB_DIR=<dir> skip building; link the libkiln.so (libkiln.dylib on macOS) in <dir>
//
// The built library is copied next to the binaries (target/<profile>/), and the binaries
// look for it beside themselves and in ../lib and ../lib/infind (on macOS also in an app
// bundle's ../Frameworks), so a plain `cargo build` runs from the target folder and an
// installed or unpacked copy runs from wherever it is put.
use std::path::{Path, PathBuf};
use std::process::Command;

/// libkiln.so* on Linux, libkiln*.dylib on macOS.
fn is_kiln_lib(name: &str) -> bool {
    if macos() {
        name.starts_with("libkiln") && name.ends_with(".dylib")
    } else {
        name.starts_with("libkiln.so")
    }
}

fn macos() -> bool {
    std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
}

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
                if is_kiln_lib(&name) {
                    let _ = std::fs::copy(e.path(), profile_dir.join(&name));
                }
            }
        }
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=kiln");
    let here = if macos() { "@executable_path" } else { "$ORIGIN" };
    let mut rpaths = vec![here.to_string(), format!("{here}/../lib"), format!("{here}/../lib/infind")];
    if macos() {
        rpaths.push(format!("{here}/../Frameworks"));
    }
    rpaths.push(lib_dir.display().to_string());
    for r in rpaths {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{r}");
    }
}

fn build_kiln(src: &Path, out: &Path) -> PathBuf {
    if !src.join("CMakeLists.txt").exists() {
        panic!(
            "kiln sources not found at {}.\n\
             Fetch them with:  git submodule update --init --recursive\n\
             or point KILN_DIR at a kiln checkout, or KILN_LIB_DIR at a built libkiln.",
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
    // kiln's src/probe/fast_cores.h uses size_t without including <cstddef>. GCC 13 lets it
    // pass, GCC 11 (Ubuntu 22.04) does not. Forcing the include is harmless everywhere on
    // Linux and keeps old compilers working until kiln adds the line itself.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let mut flags = std::env::var("CXXFLAGS").unwrap_or_default();
        if !flags.contains("cstddef") {
            flags.push_str(" -include cstddef");
        }
        cfg.env("CXXFLAGS", flags.trim());
    }
    cfg.args(["-DCMAKE_BUILD_TYPE=Release", "-DKILN_BUILD_CLI=OFF", "-DKILN_BUILD_TESTS=OFF"]);
    if let Ok(extra) = std::env::var("KILN_CMAKE_ARGS") {
        cfg.args(extra.split_whitespace());
    }
    run(&mut cfg, "configure kiln (is cmake installed?)");

    let jobs = std::env::var("NUM_JOBS").unwrap_or_else(|_| "4".into());
    let mut b = Command::new("cmake");
    b.arg("--build").arg(&build).args(["--target", "kiln", "--config", "Release", "-j", &jobs]);
    run(&mut b, "build kiln");

    let lib = if macos() { "libkiln.dylib" } else { "libkiln.so" };
    for cand in [build.clone(), build.join("Release")] {
        if cand.join(lib).exists() {
            return cand;
        }
    }
    panic!("kiln built, but {lib} was not found under {}", build.display());
}

fn run(cmd: &mut Command, what: &str) {
    let st = cmd.status().unwrap_or_else(|e| panic!("could not {what}: {e}"));
    if !st.success() {
        panic!("failed to {what}");
    }
}
