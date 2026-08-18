use std::process::Command;

#[test]
fn binary_emits_the_registry_manifest_as_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_harness-e2e"))
        .arg("--manifest")
        .output()
        .expect("spawn harness-e2e --manifest");

    assert!(
        output.status.success(),
        "manifest command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("manifest output is JSON");
    assert_eq!(manifest["name"], "harness-e2e");
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert!(manifest["description"].is_string());
    assert!(manifest["default_config"].is_object());
    assert!(!manifest["supported_targets"]
        .as_array()
        .expect("supported_targets is an array")
        .is_empty());
}
