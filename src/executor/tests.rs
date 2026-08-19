use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::{Executor, ExecutorError};
use crate::{
    builtin::{self, BuiltinFn, BuiltinIo, BuiltinOutput},
    lexer::Lexer,
    parser::Parser,
    shell::Shell,
};

static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let sequence = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "codecrafters-shell-executor-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&dir).expect("test directory should be created");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn failing_builtin(_shell: &mut Shell, _args: &[String], _io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    Ok(7)
}

fn shell<const N: usize>(current_dir: &Path, builtins: [(&str, BuiltinFn); N]) -> Shell {
    Shell::new(current_dir, HashMap::new(), builtins)
}

fn shell_with_system_path(current_dir: &Path) -> Shell {
    let path_env = std::env::var("PATH").expect("test process should define PATH");
    Shell::new(
        current_dir,
        HashMap::from([("PATH".to_owned(), path_env)]),
        [] as [(&str, BuiltinFn); 0],
    )
}

fn write_executable_script(path: &Path, marker: &str) {
    fs::write(path, format!("#!/bin/sh\nprintf '{marker}'\n"))
        .expect("executable fixture should be written");
    let mut permissions = fs::metadata(path)
        .expect("executable fixture should exist")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("fixture should be executable");
}

fn execute_line(
    executor: &mut Executor,
    shell: &mut Shell,
    source: &str,
) -> Result<i32, ExecutorError> {
    let tokens = Lexer::new(source).lex().expect("test input should lex");
    let ast = Parser::new(tokens)
        .parse()
        .expect("test input should parse")
        .expect("test input should produce an AST");
    executor.execute(shell, ast)
}

#[test]
fn executes_a_builtin_with_output_redirection() {
    let test_dir = TestDir::new();
    fs::write(test_dir.path().join("output.txt"), "stale content\n")
        .expect("fixture should be written");
    let mut shell = shell(test_dir.path(), [("echo", builtin::echo as BuiltinFn)]);
    let mut executor = Executor::new();

    let status = execute_line(&mut executor, &mut shell, "echo hello world > output.txt")
        .expect("builtin should execute");

    assert_eq!(status, 0);
    assert_eq!(
        fs::read_to_string(test_dir.path().join("output.txt"))
            .expect("redirected output should exist"),
        "hello world\n"
    );
}

#[test]
fn output_append_preserves_existing_content() {
    let test_dir = TestDir::new();
    let output = test_dir.path().join("output.txt");
    fs::write(&output, "first\n").expect("fixture should be written");
    let mut shell = shell(test_dir.path(), [("echo", builtin::echo as BuiltinFn)]);
    let mut executor = Executor::new();

    execute_line(&mut executor, &mut shell, "echo second >> output.txt")
        .expect("builtin should execute");

    assert_eq!(
        fs::read_to_string(output).expect("redirected output should exist"),
        "first\nsecond\n"
    );
}

#[test]
fn and_if_executes_the_right_side_only_after_success() {
    let test_dir = TestDir::new();
    let builtins = [
        ("fail", failing_builtin as BuiltinFn),
        ("exit", builtin::exit as BuiltinFn),
    ];
    let mut executor = Executor::new();
    let mut failed_shell = shell(test_dir.path(), builtins);

    let status = execute_line(&mut executor, &mut failed_shell, "fail && exit")
        .expect("and-if should execute");
    assert_eq!(status, 7);
    assert!(!failed_shell.exit_requested());

    let mut successful_shell = shell(test_dir.path(), [("exit", builtin::exit as BuiltinFn)]);
    let status = execute_line(&mut executor, &mut successful_shell, "> created && exit")
        .expect("and-if should execute");
    assert_eq!(status, 0);
    assert!(successful_shell.exit_requested());
    assert!(test_dir.path().join("created").is_file());
}

#[test]
fn or_if_executes_the_right_side_only_after_failure() {
    let test_dir = TestDir::new();
    let builtins = [
        ("fail", failing_builtin as BuiltinFn),
        ("exit", builtin::exit as BuiltinFn),
    ];
    let mut executor = Executor::new();
    let mut failed_shell = shell(test_dir.path(), builtins);

    let status = execute_line(&mut executor, &mut failed_shell, "fail || exit")
        .expect("or-if should execute");
    assert_eq!(status, 0);
    assert!(failed_shell.exit_requested());

    let mut successful_shell = shell(test_dir.path(), [("exit", builtin::exit as BuiltinFn)]);
    let status = execute_line(&mut executor, &mut successful_shell, "> created || exit")
        .expect("or-if should execute");
    assert_eq!(status, 0);
    assert!(!successful_shell.exit_requested());
    assert!(test_dir.path().join("created").is_file());
}

#[test]
fn pipeline_passes_output_to_the_next_stage() {
    let test_dir = TestDir::new();
    let mut shell = shell_with_system_path(test_dir.path());
    let mut executor = Executor::new();

    let status = execute_line(
        &mut executor,
        &mut shell,
        "printf pipeline-output | cat > output.txt",
    )
    .expect("pipeline should execute");

    assert_eq!(status, 0);
    assert_eq!(
        fs::read_to_string(test_dir.path().join("output.txt"))
            .expect("pipeline output should exist"),
        "pipeline-output"
    );
}

#[test]
fn command_redirection_overrides_the_pipeline_output() {
    let test_dir = TestDir::new();
    let mut shell = shell_with_system_path(test_dir.path());
    let mut executor = Executor::new();

    let status = execute_line(
        &mut executor,
        &mut shell,
        "printf redirected > direct.txt | cat > pipeline.txt",
    )
    .expect("pipeline should execute");

    assert_eq!(status, 0);
    assert_eq!(
        fs::read_to_string(test_dir.path().join("direct.txt"))
            .expect("redirected output should exist"),
        "redirected"
    );
    assert_eq!(
        fs::read_to_string(test_dir.path().join("pipeline.txt"))
            .expect("pipeline output should exist"),
        ""
    );
}

#[test]
fn pipeline_returns_the_last_stage_status() {
    let test_dir = TestDir::new();
    let mut shell = shell_with_system_path(test_dir.path());
    let mut executor = Executor::new();

    let status =
        execute_line(&mut executor, &mut shell, "false | true").expect("pipeline should execute");
    assert_eq!(status, 0);

    let status =
        execute_line(&mut executor, &mut shell, "true | false").expect("pipeline should execute");
    assert_ne!(status, 0);
}

#[test]
// https://github.com/jinyuanZhou-Leo/jsh/issues/1
fn later_stage_redirection_failure_does_not_leave_a_running_child() {
    let test_dir = TestDir::new();
    let started = test_dir.path().join("started.txt");
    let finished = test_dir.path().join("finished.txt");
    let missing = test_dir.path().join("missing.txt");
    let mut shell = shell_with_system_path(test_dir.path());
    let mut executor = Executor::new();

    let error = execute_line(
        &mut executor,
        &mut shell,
        "sh -c 'printf started > started.txt; sleep 1; printf finished > finished.txt' | cat < missing.txt",
    )
    .expect_err("later-stage input redirection should fail during setup");

    assert!(matches!(
        error,
        ExecutorError::OpenRedirection { path, .. } if path == missing
    ));
    assert!(
        !started.exists(),
        "first pipeline stage should not start when a later stage fails to prepare"
    );
    assert!(!finished.exists());

    thread::sleep(Duration::from_secs(2));

    assert!(
        !started.exists(),
        "an unreaped first-stage child would have created started.txt"
    );
    assert!(
        !finished.exists(),
        "an unreaped first-stage child would have created finished.txt after sleeping"
    );
}

#[test]
fn command_not_found_uses_status_127_and_obeys_stderr_redirection() {
    let test_dir = TestDir::new();
    let mut shell = shell(test_dir.path(), []);
    let mut executor = Executor::new();

    let status = execute_line(
        &mut executor,
        &mut shell,
        "definitely-not-a-command 2> error.txt",
    )
    .expect("missing command should return a status");

    assert_eq!(status, 127);
    assert_eq!(
        fs::read_to_string(test_dir.path().join("error.txt"))
            .expect("redirected error should exist"),
        "definitely-not-a-command: not found\n"
    );
}

#[test]
fn rejects_an_unsupported_redirection_without_creating_the_target() {
    let test_dir = TestDir::new();
    let mut shell = shell(test_dir.path(), []);
    let mut executor = Executor::new();

    let error = execute_line(&mut executor, &mut shell, "3> output.txt")
        .expect_err("fd 3 should not be supported");

    assert!(matches!(
        error,
        ExecutorError::UnsupportedRedirection {
            redirected_fd: 3,
            operator: crate::token::RedirectOperator::OutputTruncate,
        }
    ));
    assert!(!test_dir.path().join("output.txt").exists());
}

#[test]
// https://github.com/jinyuanZhou-Leo/jsh/issues/4
fn relative_executable_is_resolved_against_the_shell_cwd_after_cd() {
    let start_dir = TestDir::new();
    let script_dir = TestDir::new();
    let script = script_dir.path().join("jsh-relative-script");
    write_executable_script(&script, "from-logical-cwd");

    let mut shell = shell(start_dir.path(), [("cd", builtin::cd as BuiltinFn)]);
    let mut executor = Executor::new();

    let status = execute_line(
        &mut executor,
        &mut shell,
        &format!(
            "cd {} && ./jsh-relative-script > output.txt",
            script_dir.path().display()
        ),
    )
    .expect("relative executable should run after cd");

    assert_eq!(status, 0);
    assert_eq!(
        fs::read_to_string(script_dir.path().join("output.txt"))
            .expect("script output should be redirected"),
        "from-logical-cwd"
    );
    assert!(
        !start_dir.path().join("output.txt").exists(),
        "redirection should use the shell current directory after cd, not the original directory"
    );
}

#[test]
// https://github.com/jinyuanZhou-Leo/jsh/issues/4
fn absolute_executable_path_runs_from_another_shell_cwd() {
    let start_dir = TestDir::new();
    let script_dir = TestDir::new();
    let script = script_dir.path().join("jsh-absolute-script");
    write_executable_script(&script, "from-absolute-path");

    let mut shell = shell(start_dir.path(), []);
    let mut executor = Executor::new();

    let status = execute_line(
        &mut executor,
        &mut shell,
        &format!("{} > output.txt", script.display()),
    )
    .expect("absolute executable should run");

    assert_eq!(status, 0);
    assert_eq!(
        fs::read_to_string(start_dir.path().join("output.txt"))
            .expect("script output should be redirected"),
        "from-absolute-path"
    );
}

#[test]
// https://github.com/jinyuanZhou-Leo/jsh/issues/4
fn relative_path_dir_is_resolved_against_the_shell_current_dir() {
    let logical_dir = TestDir::new();
    let bin_dir = logical_dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("relative PATH directory should be created");
    write_executable_script(&bin_dir.join("jsh-path-tool"), "from-relative-path");

    let mut shell = Shell::new(
        logical_dir.path(),
        HashMap::from([("PATH".to_owned(), "bin".to_owned())]),
        [] as [(&str, BuiltinFn); 0],
    );
    let mut executor = Executor::new();

    let status = execute_line(&mut executor, &mut shell, "jsh-path-tool > output.txt")
        .expect("command in a relative PATH directory should run");

    assert_eq!(status, 0);
    assert_eq!(
        fs::read_to_string(logical_dir.path().join("output.txt"))
            .expect("script output should be redirected"),
        "from-relative-path"
    );
}

#[test]
fn missing_input_file_reports_the_resolved_target_path() {
    let test_dir = TestDir::new();
    let expected = test_dir.path().join("missing.txt");
    let mut shell = shell(test_dir.path(), []);
    let mut executor = Executor::new();

    let error = execute_line(&mut executor, &mut shell, "< missing.txt")
        .expect_err("missing input should fail");

    assert!(matches!(
        error,
        ExecutorError::OpenRedirection { path, .. } if path == expected
    ));
}
