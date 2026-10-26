//! Builds SVT-AV1 from the pinned submodule at `third_party/SVT-AV1`, and generates the bindings
//! for its encoder API.
//!
//! The source is the commit the submodule records, v4.2.0. CMake comes from `shiguredo_cmake`,
//! which downloads a pinned, SHA-256 checked CMake, unless the `CMAKE` variable is already set.
//! NASM is used for the x86 assembly, and libclang for bindgen.

use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    // Cargo passes each enabled feature to the build script as an environment variable.
    if std::env::var_os("CARGO_FEATURE_BUILD").is_none() {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"));
    let src = manifest.join("../../third_party/SVT-AV1");
    println!("cargo::rerun-if-env-changed=CMAKE");

    if !src.join("CMakeLists.txt").exists() {
        panic!(
            "SVT-AV1 is missing at {}. Run `git submodule update --init third_party/SVT-AV1`.",
            src.display()
        );
    }

    shiguredo_cmake::set_cmake_env();
    let dst = cmake::Config::new(&src)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("SVT_AV1_LTO", "OFF")
        .define("BUILD_TESTING", "OFF")
        .define("BUILD_APPS", "OFF")
        .profile("Release")
        .build();

    println!("cargo::rustc-link-search=native={}", dst.join("lib").display());
    println!("cargo::rustc-link-search=native={}", dst.join("lib64").display());
    println!("cargo::rustc-link-lib=static=SvtAv1Enc");

    let api = src.join("Source/API");
    let header = api.join("EbSvtAv1Enc.h");
    let bindings = bindgen::Builder::default()
        .header(header.to_str().expect("the header path is UTF-8"))
        .clang_arg(format!("-I{}", api.display()))
        .allowlist_function("svt_av1_enc_.*")
        .allowlist_type("EbSvt.*|EbComponentType|EbBufferHeaderType|EbErrorType|EbAv1PictureType|EbColorFormat|EbSvtAv1EncConfiguration|PredStructure|SvtAv1RcMode|SvtAv1IntraRefreshType")
        .allowlist_var("EB_.*|SVT_AV1_.*")
        .derive_debug(false)
        .layout_tests(false)
        .generate()
        .expect("bindgen reads the SVT-AV1 encoder header");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("the bindings are written to OUT_DIR");
}
