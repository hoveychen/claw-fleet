fn main() {
    // libgit2-sys 0.16 (pulled in by git2) forgets to emit advapi32 on
    // Windows MSVC, so any test linking through this crate fails on
    // Crypt*/Reg*/GetNamedSecurityInfoW. Add it here.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=advapi32");
    }
}
