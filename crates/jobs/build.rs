fn main() {
    // sqlx embeds migrations at compile time; additions must invalidate cached test binaries.
    println!("cargo:rerun-if-changed=../../migrations");
}
