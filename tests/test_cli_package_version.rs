use std::process::Command;

#[test]
fn binary_and_machine_protocol_report_the_same_cli_package_version() {
    let binary = env!("CARGO_BIN_EXE_proj");
    let output = Command::new(binary).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), format!("proj {}", env!("CARGO_PKG_VERSION")));
    let output = Command::new(binary).args(["--machine", "capabilities"]).output().unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["cli_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value["protocol_version"], 1);
}
