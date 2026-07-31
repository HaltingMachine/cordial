use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let native = root.join("native");

    if !root.join("third_party/mcpelauncher-linker/bionic/linker/linker.cpp").exists() {
        panic!(
            "third_party/mcpelauncher-linker is not checked out.\n\
             Run: git submodule update --init --recursive"
        );
    }

    // AOSP bionic does not build with GCC; see docs/base-evaluation.md §2.1.
    let dst = cmake::Config::new(&native)
        .define("CMAKE_C_COMPILER", "clang")
        .define("CMAKE_CXX_COMPILER", "clang++")
        .define("CMAKE_BUILD_TYPE", "Release")
        .build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=cordial_linker_shim");
    println!("cargo:rustc-link-lib=static=cordial_jni_shim");
    println!("cargo:rustc-link-lib=static=jnivm");
    println!("cargo:rustc-link-lib=static=linker");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-lib=dylib=z");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=pthread");

    println!("cargo:rerun-if-changed={}", native.join("shim.cpp").display());
    println!("cargo:rerun-if-changed={}", native.join("jni_shim.cpp").display());
    println!("cargo:rerun-if-changed={}", native.join("CMakeLists.txt").display());
}
