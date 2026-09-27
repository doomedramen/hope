fn main() {
    // Keep embedded migrations synchronized when a migration is added without a Rust source edit.
    println!("cargo:rerun-if-changed=../../migrations");
}
