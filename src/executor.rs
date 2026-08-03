use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitStatus};

use crate::builtin::{BuiltinFn, BuiltinIo};
use crate::expander::{ExpandedRedirectOperand, ExpandedRedirection, Expander, ExpanderError};
use crate::parser::Command;
use crate::shell::ResolvedCommand;
use crate::token::RedirectOperator;
use crate::{parser::Ast, shell::Shell};
use thiserror::Error;

#[derive(Debug)]
struct PreparedIo {
    stdin: File,
    stdout: File,
    stderr: File,
}

impl PreparedIo {
    fn inherit() -> io::Result<Self> {
        Ok(Self {
            stdin: File::from(io::stdin().as_fd().try_clone_to_owned()?),
            stdout: File::from(io::stdout().as_fd().try_clone_to_owned()?),
            stderr: File::from(io::stderr().as_fd().try_clone_to_owned()?),
        })
    }

    fn try_clone_fd(&self, fd: u32) -> Result<File, ExecutorError> {
        match fd {
            0 => Ok(self.stdin.try_clone()?),
            1 => Ok(self.stdout.try_clone()?),
            2 => Ok(self.stderr.try_clone()?),
            _ => Err(ExecutorError::BadFileDescripter { fd }),
        }
    }

    fn replace_fd_with(&mut self, fd: u32, replacement: File) -> Result<(), ExecutorError> {
        match fd {
            0 => self.stdin = replacement,
            1 => self.stdout = replacement,
            2 => self.stderr = replacement,
            _ => return Err(ExecutorError::UnsupportedFileDescriptor { fd }),
        }

        Ok(())
    }

    fn stderr_writer(&self) -> io::Result<File> {
        Ok(self.stderr.try_clone()?)
    }
}

pub struct Executor;

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error(transparent)]
    Expansion(#[from] ExpanderError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("cannot open redirection target `{path}`: {source}")]
    OpenRedirection {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("unsupported redirection: fd {redirected_fd} with operator {operator:?}")]
    UnsupportedRedirection {
        redirected_fd: u32,
        operator: RedirectOperator,
    },
    #[error("bad file descriptor: {fd}")]
    BadFileDescripter { fd: u32 },
    #[error("unsupported file descriptor: {fd}")]
    UnsupportedFileDescriptor { fd: u32 },
}

impl Executor {
    pub(crate) fn new() -> Self {
        Self {}
    }

    pub(crate) fn execute(&mut self, shell: &mut Shell, ast: Ast) -> Result<i32, ExecutorError> {
        self.execute_ast(shell, ast)
    }

    pub(crate) fn execute_ast(
        &mut self,
        shell: &mut Shell,
        ast: Ast,
    ) -> Result<i32, ExecutorError> {
        match ast {
            Ast::Command(command) => self.execute_command(shell, command),
            Ast::AndIf { left, right } => {
                let status = self.execute_ast(shell, *left)?;

                // 如果上一条指令成功执行，且shell没有被要求关闭，则继续运行右侧ast
                if status == 0 && !shell.exit_requested() {
                    self.execute_ast(shell, *right)
                } else {
                    // 否则不执行右侧指令，返回左侧结果
                    Ok(status)
                }
            },
            Ast::OrIf { left, right } => {
                let status = self.execute_ast(shell, *left)?;

                // 如果左侧指令没有成功执行, 且shell没有被要求关闭， 则继续执行右侧ast
                if status != 0 && !shell.exit_requested() {
                    self.execute_ast(shell, *right)
                } else {
                    Ok(status)
                }
            },
            Ast::Pipeline { commands } => {
                todo!()
            }
        }
    }

    pub(crate) fn execute_command(
        &mut self,
        shell: &mut Shell,
        command: Command,
    ) -> Result<i32, ExecutorError> {
        let expanded_command = Expander::new(shell.environment()).expand_command(command)?;

        // 重定向需要在命令查找之前完成
        let prepared_io = self.prepare_io(shell.current_dir(), expanded_command.redirections)?;

        // 只要存在重定向，命令为空也是合法的
        let Some((command_name, argv)) = expanded_command.args.split_first() else {
            // 空指令
            return Ok(0);
        };

        match shell.resolve_command(command_name) {
            Some(ResolvedCommand::Builtin(builtin)) => {
                self.execute_builtin(builtin, shell, argv, prepared_io)
            }
            Some(ResolvedCommand::External(path)) => {
                self.execute_external(&path, command_name, shell, argv, prepared_io)
            }
            None => {
                // Command Not Found
                writeln!(prepared_io.stderr_writer()?, "{}: not found", command_name)?;
                Ok(127)
            }
        }
    }

    fn execute_builtin(
        &self,
        builtin: BuiltinFn,
        shell: &mut Shell,
        argv: &[String],
        mut prepared_io: PreparedIo,
    ) -> Result<i32, ExecutorError> {
        let mut io = BuiltinIo::new(&mut prepared_io.stdin, &mut prepared_io.stdout, &mut prepared_io.stderr);

        match builtin(shell, argv, &mut io) {
            Ok(code) => Ok(code),
            Err(error) => {
                let status = error.status();

                // 此处同样遵守重定向
                writeln!(io.stderr(), "{error}")?;
                Ok(status)
            }
        }
    }

    fn execute_external(
        &self,
        executable: &Path,
        command_name: &String,
        shell: &mut Shell,
        argv: &[String],
        prepared_io: PreparedIo,
    ) -> Result<i32, ExecutorError> {
        // 创建一个用于记录错误的stderr_writer
        let mut logger = prepared_io.stderr_writer()?;

        let result = ProcessCommand::new(executable)
            .arg0(command_name)
            .args(argv)
            .env_clear()
            .envs(shell.environment())
            .stdin(prepared_io.stdin)
            .stdout(prepared_io.stdout)
            .stderr(prepared_io.stderr)
            .status();

        match result {
            Ok(status) => Ok(Self::exit_status_code(status)),
            Err(error) => {
                writeln!(
                    logger,
                    "{}: failed to execute: {error}",
                    executable.display()
                )?;

                // 命令找到但无法执行，对应状态码126
                Ok(126)
            }
        }
    }

    /// 准备重定向，创建文件句柄
    fn prepare_io(
        &self,
        current_dir: &Path,
        redirections: Vec<ExpandedRedirection>,
    ) -> Result<PreparedIo, ExecutorError> {
        let mut prepared_io = PreparedIo::inherit()?;

        // 按照顺序处理重定向
        // 靠后的重定向会覆盖前面的重定向
        for redirection in redirections {
            match (
                redirection.redirected_fd,
                redirection.operator,
                redirection.operand,
            ) {
                (
                    destination_fd,
                    RedirectOperator::DuplicateInput | RedirectOperator::DuplicateOutput,
                    ExpandedRedirectOperand::Fd(source_fd),
                ) => {
                    prepared_io
                        .replace_fd_with(destination_fd, prepared_io.try_clone_fd(source_fd)?)?;
                }
                (0, RedirectOperator::Input, ExpandedRedirectOperand::Path(path)) => {
                    prepared_io.replace_fd_with(
                        0,
                        Self::open_input(&self.resolve_redirection_path(current_dir, &path))?,
                    )?;
                }
                (1, RedirectOperator::OutputTruncate, ExpandedRedirectOperand::Path(path)) => {
                    prepared_io.replace_fd_with(
                        1,
                        Self::open_output_truncate(
                            &self.resolve_redirection_path(current_dir, &path),
                        )?,
                    )?;
                }
                (2, RedirectOperator::OutputTruncate, ExpandedRedirectOperand::Path(path)) => {
                    prepared_io.replace_fd_with(
                        2,
                        Self::open_output_truncate(
                            &self.resolve_redirection_path(current_dir, &path),
                        )?,
                    )?;
                }
                (1, RedirectOperator::OutputAppend, ExpandedRedirectOperand::Path(path)) => {
                    prepared_io.replace_fd_with(
                        1,
                        Self::open_output_append(
                            &self.resolve_redirection_path(current_dir, &path),
                        )?,
                    )?;
                }
                (2, RedirectOperator::OutputAppend, ExpandedRedirectOperand::Path(path)) => {
                    prepared_io.replace_fd_with(
                        2,
                        Self::open_output_append(
                            &self.resolve_redirection_path(current_dir, &path),
                        )?,
                    )?;
                }
                (redirected_fd, operator, _) => {
                    return Err(ExecutorError::UnsupportedRedirection {
                        redirected_fd,
                        operator,
                    });
                }
            }
        }

        Ok(prepared_io)
    }

    /// 将重定向路径转换成绝对路径
    ///
    /// # Arguments
    ///
    /// - `current_dir` (`&Path`) - shell当前运行目录
    /// - `redirection_path` (`&str`) - 重定向路径
    ///
    /// # Returns
    ///
    /// - `PathBuf` - 重定向目标的绝对路径
    /// ```
    fn resolve_redirection_path(&self, current_dir: &Path, redirection_path: &str) -> PathBuf {
        let path = Path::new(redirection_path);

        if path.is_absolute() {
            PathBuf::from(path)
        } else {
            current_dir.join(path)
        }
    }

    /// 获取一个input模式的File句柄
    fn open_input(path: &Path) -> Result<File, ExecutorError> {
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|source| ExecutorError::OpenRedirection {
                path: path.to_path_buf(),
                source,
            })
    }

    /// 获取一个output truncate模式的File句柄
    fn open_output_truncate(path: &Path) -> Result<File, ExecutorError> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|source| ExecutorError::OpenRedirection {
                path: path.to_path_buf(),
                source,
            })
    }

    /// 获取一个output append模式的File句柄
    fn open_output_append(path: &Path) -> Result<File, ExecutorError> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| ExecutorError::OpenRedirection {
                path: path.to_path_buf(),
                source,
            })
    }

    /// 处理external executable执行返回状态码的工具函数
    fn exit_status_code(status: ExitStatus) -> i32 {
        // 如果存在状态码则直接返回
        if let Some(code) = status.code() {
            return code;
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            // 如果进程不是正常退出，而是被Unix信号终止
            if let Some(signal) = status.signal() {
                return 128 + signal;
            }
        }

        1
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
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
            let path = std::env::temp_dir().join(format!(
                "codecrafters-shell-executor-{}-{timestamp}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("test directory should be created");
            Self(path)
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

    fn failing_builtin(
        _shell: &mut Shell,
        _args: &[String],
        _io: &mut BuiltinIo<'_>,
    ) -> BuiltinOutput {
        Ok(7)
    }

    fn shell<const N: usize>(current_dir: &Path, builtins: [(&str, BuiltinFn); N]) -> Shell {
        Shell::new(current_dir, HashMap::new(), builtins)
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
}
