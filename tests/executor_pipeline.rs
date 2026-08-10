use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn builtin_output_flows_through_an_external_pipeline_stage() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_codecrafters-shell"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("shell process should start");

    child
        .stdin
        .take()
        .expect("shell stdin should be piped")
        .write_all(b"echo builtin-pipeline-output | cat\nexit\n")
        .expect("commands should be written");

    let output = child.wait_with_output().expect("shell process should exit");
    let stdout = String::from_utf8(output.stdout).expect("shell output should be UTF-8");

    assert!(output.status.success());
    assert!(
        stdout
            .lines()
            .any(|line| line == "$ builtin-pipeline-output"),
        "builtin output should reach the external pipeline stage: {stdout:?}"
    );
    assert!(output.stderr.is_empty());
}
