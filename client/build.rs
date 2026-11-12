//! Makes the client look for shared libraries (Assimp) in its own folder,
//! so the executable and `libassimp` can be shipped side by side.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN"),
        Ok("macos") => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path"),
        _ => {}
    }
}
