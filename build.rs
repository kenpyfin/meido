fn main() {
    // Rebuild when embedded frontend assets change.
    println!("cargo:rerun-if-changed=web/dist");
    println!("cargo:rerun-if-changed=builtin_skills");
}
