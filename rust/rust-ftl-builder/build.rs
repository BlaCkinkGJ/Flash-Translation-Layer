extern crate bindgen;
extern crate pkg_config;

use std::env;

fn main() {
    println!("cargo:rustc-link-search=/usr/local/lib");

    println!("cargo:rustc-link-lib=bz2");

    println!("cargo:rerun-if-changed=wrapper.h");

    let mut builder = bindgen::Builder::default().header("wrapper.h");

    // include glib-2.0
    let library = pkg_config::probe_library("glib-2.0").unwrap();
    for path in library.include_paths {
        builder = builder.clang_arg(format!("-I{}", path.display()));
    }

    // include ftl library
    builder = builder.clang_arg(format!("-I{}", env::var("FTL_INCLUDE_PATH").unwrap()));

    // create bindings
    let bindings = builder
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    bindings
        .write_to_file("bindings.rs")
        .expect("Couldn't write bindings!");
}
