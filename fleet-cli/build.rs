fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=FLEET_BUILD_VERSION");

    // FLEET_BUILD_VERSION lets build-local.sh stamp a dev version without
    // editing Cargo.toml; absent it, fall back to the crate's own version.
    let version = std::env::var("FLEET_BUILD_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into()));
    println!("cargo:rustc-env=FLEET_BUILD_VERSION={version}");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let plist_path = std::path::Path::new(&out_dir).join("Info.plist");

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>English</string>
    <key>CFBundleExecutable</key>
    <string>fleet</string>
    <key>CFBundleIdentifier</key>
    <string>com.hoveychen.claw-fleet.serve</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>Claw Fleet</string>
    <key>CFBundleDisplayName</key>
    <string>Claw Fleet</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>CFBundleIconFile</key>
    <string>icon.icns</string>
    <key>LSUIElement</key>
    <true/>
</dict>
</plist>
"#
    );
    std::fs::write(&plist_path, plist).expect("write Info.plist");

    let plist_str = plist_path.to_string_lossy();
    println!("cargo:rustc-link-arg-bin=fleet-cli=-Wl,-sectcreate,__TEXT,__info_plist,{plist_str}");
}
