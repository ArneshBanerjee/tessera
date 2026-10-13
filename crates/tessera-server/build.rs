//! Rebuilds the server when the web app is rebuilt, so the embedded copy stays current.

fn main() {
    println!("cargo:rerun-if-changed=../tessera-web/dist");
}
