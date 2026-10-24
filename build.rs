fn main() {
    println!("cargo:rerun-if-env-changed=DOCS_RS");
    // Rustdoc needs the Rust API, not a compiled or linked native SDK.
    if std::env::var_os("DOCS_RS").is_some() {
        return;
    }
    println!("cargo:rerun-if-env-changed=CESIUM_NATIVE_DIR");
    {
        let root = native_install();
        assert!(
            root.join("include/Cesium3DTilesSelection/Tileset.h")
                .is_file(),
            "Build Cesium Native with scripts/build_cesium_native.sh in bevy-cesium, or set CESIUM_NATIVE_DIR to an existing 0.65 installation."
        );
        let mut bridge = cxx_build::bridge("src/native.rs");
        bridge
            .file("native/bridge.cpp")
            .include("native/include")
            .include(root.join("include"))
            // The supported SDK is built in Release mode. This must match:
            // Cesium's ReferenceCounted layout changes when NDEBUG is absent.
            .define("NDEBUG", None)
            .std("c++20");
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            bridge
                .flag("/utf-8")
                .flag("/EHsc")
                // Native headers need C++20, but async++ 1.2 still uses result_of.
                .define("_HAS_DEPRECATED_RESULT_OF", "1")
                // async++ includes Windows headers. WIN32_LEAN_AND_MEAN does
                // not exclude GDI's OPAQUE macro, which conflicts with glTF.
                .define("WIN32_LEAN_AND_MEAN", None)
                .define("NOGDI", None)
                .define("NOMINMAX", None);
        }
        bridge.compile("cesium-bridge");
        println!("cargo:rerun-if-changed=src");
        println!("cargo:rerun-if-changed=native");
        println!(
            "cargo:rustc-link-search=native={}",
            root.join("lib").display()
        );
        let mut libraries: Vec<_> = std::fs::read_dir(root.join("lib"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext == "a" || ext == "lib")
            })
            .collect();
        libraries.sort();
        for library in libraries {
            let stem = library.file_stem().unwrap().to_str().unwrap();
            let name = if library.extension().is_some_and(|ext| ext == "a") {
                stem.strip_prefix("lib").unwrap_or(stem)
            } else {
                stem
            };
            println!("cargo:rustc-link-lib=static={name}");
        }
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            // System dependencies of the static curl/OpenSSL libraries.
            for library in [
                "crypt32", "secur32", "ws2_32", "wldap32", "normaliz", "bcrypt",
            ] {
                println!("cargo:rustc-link-lib={library}");
            }
        }
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
            for framework in ["Security", "SystemConfiguration", "CoreFoundation"] {
                println!("cargo:rustc-link-lib=framework={framework}");
            }
            println!("cargo:rustc-link-lib=iconv");
            println!("cargo:rustc-link-lib=resolv");
        }
    }
}

fn native_install() -> std::path::PathBuf {
    use std::{env, path::PathBuf};
    for key in ["CESIUM_BUILD_ROOT", "XDG_CACHE_HOME", "HOME", "USERPROFILE"] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    if let Some(path) = env::var_os("CESIUM_NATIVE_DIR") {
        return PathBuf::from(path);
    }
    let root = env::var_os("CESIUM_BUILD_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let cache = env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(
                        env::var_os("HOME")
                            .or_else(|| env::var_os("USERPROFILE"))
                            .expect("Set CESIUM_NATIVE_DIR when no home directory is available"),
                    )
                    .join(".cache")
                });
            cache.join("bevy-cesium/cesium-0.65.0")
        });
    root.join("install")
}
