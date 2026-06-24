// Emit the target triple this binary is built for, so self-update (6h) can pick the right artifact
// from the signed release manifest. Cargo sets TARGET for build scripts. Zero dependencies.
fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=SA_TARGET={target}");
}
