fn main() {
    println!("cargo:rerun-if-changed=assets/brand/garage.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let icon = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("assets/brand/garage.ico");
    let rc = out.join("garage.rc");
    std::fs::write(
        &rc,
        format!(
            "1 ICON \"{}\"\n",
            icon.display().to_string().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    let resource = out.join("garage.res");
    let status = std::process::Command::new("rc.exe")
        .arg("/nologo")
        .arg(format!("/fo{}", resource.display()))
        .arg(&rc)
        .status()
        .expect("Windows SDK rc.exe is required; run from a Visual Studio developer shell");
    assert!(status.success(), "Windows icon resource compilation failed");
    println!("cargo:rustc-link-arg={}", resource.display());
}
