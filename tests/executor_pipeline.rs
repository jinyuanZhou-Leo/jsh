use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nix::{sys::signal::Signal, unistd::Pid};

#[test]
fn builtin_output_flows_through_an_external_pipeline_stage() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jsh"))
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

#[test]
fn builtin_pipeline_uses_the_shell_current_directory() {
    let expected_dir = std::env::temp_dir()
        .canonicalize()
        .expect("temporary directory should exist");
    let command = format!("cd {}\npwd | cat\nexit\n", expected_dir.display());

    let mut child = Command::new(env!("CARGO_BIN_EXE_jsh"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("shell process should start");

    child
        .stdin
        .take()
        .expect("shell stdin should be piped")
        .write_all(command.as_bytes())
        .expect("commands should be written");

    let output = child.wait_with_output().expect("shell process should exit");
    let stdout = String::from_utf8(output.stdout).expect("shell output should be UTF-8");

    assert!(output.status.success());
    assert!(
        stdout.contains(&format!("$ {}", expected_dir.display())),
        "builtin pipeline should use the shell current directory: {stdout:?}"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn builtin_pipeline_does_not_reexec_the_shell_binary() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let test_dir =
        std::env::temp_dir().join(format!("jsh-fork-direct-{}-{unique}", std::process::id()));
    fs::create_dir(&test_dir).expect("test directory should be created");
    let copied_shell = test_dir.join("jsh");
    fs::copy(env!("CARGO_BIN_EXE_jsh"), &copied_shell).expect("shell binary should be copied");

    let mut child = Command::new(&copied_shell)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("copied shell process should start");

    // A fork-direct builtin no longer needs to execute the jsh binary after the parent starts.
    let mut permissions = fs::metadata(&copied_shell)
        .expect("copied shell should exist")
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(&copied_shell, permissions)
        .expect("shell execute permission should be removed");
    child
        .stdin
        .take()
        .expect("shell stdin should be piped")
        .write_all(b"echo fork-direct | cat\nexit\n")
        .expect("commands should be written");

    let output = child.wait_with_output().expect("shell process should exit");
    fs::remove_file(&copied_shell).expect("copied shell should be removed");
    fs::remove_dir(&test_dir).expect("test directory should be removed");
    let stdout = String::from_utf8(output.stdout).expect("shell output should be UTF-8");

    assert!(
        output.status.success(),
        "shell failed with status {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.lines().any(|line| line == "$ fork-direct"),
        "builtin should execute without re-execing jsh: {stdout:?}"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn builtin_closes_unrelated_pipe_fds_before_writing() {
    let payload = "x".repeat(512 * 1024);
    let command = format!("echo {payload} | head -c 1\nexit\n");
    let mut child = Command::new(env!("CARGO_BIN_EXE_jsh"))
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("shell process should start");
    let child_pid = Pid::from_raw(child.id() as i32);

    child
        .stdin
        .take()
        .expect("shell stdin should be piped")
        .write_all(command.as_bytes())
        .expect("commands should be written");

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("shell status should be readable") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = nix::sys::signal::killpg(child_pid, Signal::SIGKILL);
            let _ = child.wait();
            panic!("pipeline blocked because the builtin retained an unrelated pipe fd");
        }
        thread::sleep(Duration::from_millis(10));
    };

    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("shell stdout should be piped")
        .read_to_string(&mut stdout)
        .expect("shell stdout should be readable");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("shell stderr should be piped")
        .read_to_string(&mut stderr)
        .expect("shell stderr should be readable");

    assert!(status.success(), "shell stderr: {stderr}");
    assert!(
        stdout.contains("$ x"),
        "downstream stage output: {stdout:?}"
    );
    assert!(
        !stderr.contains("failed to install pipeline builtin I/O")
            && !stderr.contains("failed to prepare pipeline builtin"),
        "fork-direct child setup failed: {stderr}"
    );
}
