// La chaîne Rust « GNU » autonome n'a pas toutes les bibliothèques d'import Windows :
// ld sait lier directement une DLL, on lui donne donc une copie des DLL système manquantes.
fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let system = std::path::Path::new(r"C:\Windows\System32");
    for dll in ["shlwapi.dll"] {
        let _ = std::fs::copy(system.join(dll), out.join(dll));
    }
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");
}
