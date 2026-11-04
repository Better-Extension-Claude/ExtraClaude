//! Liste les extensions que BetterExt voit (copiées et dossiers surveillés).
fn main() {
    for e in betterext::installed() {
        println!("{}\t{}\tlinked={}\t{}", e.manifest.id, e.manifest.name, e.linked, e.exe_path().display());
    }
}
