//! The updater runs a downloaded copy with `--version` and installs it only if the answer is
//! `<slug> <version>` (fastframe-update's contract).

#[test]
fn version_prints_the_slug_and_version() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_tidash"))
        .arg("--version")
        .output()
        .expect("tidash runs");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        concat!("tidash ", env!("CARGO_PKG_VERSION"))
    );
}
