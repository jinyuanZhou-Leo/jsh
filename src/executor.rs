use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, PipeReader, PipeWriter, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, ExitStatus};
use std::thread::{self, JoinHandle};

use crate::builtin::{self, BuiltinError, BuiltinFn, BuiltinIo};
use crate::expander::{
    ExpandedCommand, ExpandedRedirectOperand, ExpandedRedirection, Expander, ExpanderError,
};
use crate::parser::Command;
use crate::shell::ResolvedCommand;
use crate::token::RedirectOperator;
use crate::{parser::Ast, shell::Shell};
use thiserror::Error;

#[derive(Debug)]
struct CommandIoCtx {
    stdin: File,
    stdout: File,
    stderr: File,
}

impl CommandIoCtx {
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

    /// Applies redirections in source order, so later redirections override earlier ones.
    fn apply_redirections(
        mut self,
        current_dir: &Path,
        redirections: &[ExpandedRedirection],
    ) -> Result<CommandIoCtx, ExecutorError> {
        for redirection in redirections {
            match (
                redirection.redirected_fd,
                redirection.operator,
                &redirection.operand,
            ) {
                (
                    destination_fd,
                    RedirectOperator::DuplicateInput | RedirectOperator::DuplicateOutput,
                    ExpandedRedirectOperand::Fd(source_fd),
                ) => {
                    let replacement = self.try_clone_fd(*source_fd)?;
                    self.replace_fd_with(destination_fd, replacement)?;
                }
                (0, RedirectOperator::Input, ExpandedRedirectOperand::Path(path)) => {
                    let path = Self::resolve_redirection_path(current_dir, path);
                    self.replace_fd_with(0, Self::open_input(&path)?)?;
                }
                (1, RedirectOperator::OutputTruncate, ExpandedRedirectOperand::Path(path)) => {
                    let path = Self::resolve_redirection_path(current_dir, path);
                    self.replace_fd_with(1, Self::open_output_truncate(&path)?)?;
                }
                (2, RedirectOperator::OutputTruncate, ExpandedRedirectOperand::Path(path)) => {
                    let path = Self::resolve_redirection_path(current_dir, path);
                    self.replace_fd_with(2, Self::open_output_truncate(&path)?)?;
                }
                (1, RedirectOperator::OutputAppend, ExpandedRedirectOperand::Path(path)) => {
                    let path = Self::resolve_redirection_path(current_dir, path);
                    self.replace_fd_with(1, Self::open_output_append(&path)?)?;
                }
                (2, RedirectOperator::OutputAppend, ExpandedRedirectOperand::Path(path)) => {
                    let path = Self::resolve_redirection_path(current_dir, path);
                    self.replace_fd_with(2, Self::open_output_append(&path)?)?;
                }
                (redirected_fd, operator, _) => {
                    return Err(ExecutorError::UnsupportedRedirection {
                        redirected_fd,
                        operator,
                    });
                }
            }
        }

        Ok(self)
    }

    fn resolve_redirection_path(current_dir: &Path, redirection_path: &str) -> PathBuf {
        let path = Path::new(redirection_path);

        if path.is_absolute() {
            path.to_path_buf()
        } else {
            current_dir.join(path)
        }
    }

    fn open_input(path: &Path) -> Result<File, ExecutorError> {
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|source| ExecutorError::OpenRedirection {
                path: path.to_path_buf(),
                source,
            })
    }

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
}

enum PipelineJob {
    Completed(i32),
    Process(Child),
}

impl PipelineJob {
    fn wait(self) -> Result<i32, ExecutorError> {
        match self {
            Self::Completed(status) => Ok(status),
            Self::Process(mut child) => {
                child
                    .wait()
                    .map(Executor::exit_status_code)
                    .map_err(ExecutorError::WaitPipelineProcess)
            }
        }
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

    #[error("failed to wait for pipeline process")]
    WaitPipelineProcess(#[source] io::Error),
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
            }
            Ast::OrIf { left, right } => {
                let status = self.execute_ast(shell, *left)?;

                // 如果左侧指令没有成功执行, 且shell没有被要求关闭， 则继续执行右侧ast
                if status != 0 && !shell.exit_requested() {
                    self.execute_ast(shell, *right)
                } else {
                    Ok(status)
                }
            }
            Ast::Pipeline { commands } => self.execute_pipeline(shell, commands),
        }
    }

    pub(crate) fn execute_command(
        &mut self,
        shell: &mut Shell,
        command: Command,
    ) -> Result<i32, ExecutorError> {
        let expanded_command = self.expand_command(command, shell)?;
        // 重定向需要在命令查找之前完成，因为只要存在重定向，命令为空也是合法的
        let io_ctx = CommandIoCtx::inherit()?
            .apply_redirections(shell.current_dir(), &expanded_command.redirections)?;

        let Some((command_name, argv)) = expanded_command.args.split_first() else {
            // 空指令
            return Ok(0);
        };

        match shell.resolve_command(command_name) {
            Some(ResolvedCommand::Builtin(builtin)) => {
                self.execute_builtin(builtin, shell, argv, io_ctx)
            }
            Some(ResolvedCommand::External(path)) => {
                self.execute_external(&path, command_name, shell, argv, io_ctx)
            }
            None => {
                // Command Not Found
                writeln!(io_ctx.stderr_writer()?, "{}: not found", command_name)?;
                Ok(127)
            }
        }
    }

    fn execute_builtin(
        &self,
        builtin: BuiltinFn,
        shell: &mut Shell,
        argv: &[String],
        mut io_ctx: CommandIoCtx,
    ) -> Result<i32, ExecutorError> {
        let mut io = BuiltinIo::new(&mut io_ctx.stdin, &mut io_ctx.stdout, &mut io_ctx.stderr);

        Ok(builtin::invoke(builtin, shell, argv, &mut io)?)
    }

    fn execute_external(
        &self,
        executable: &Path,
        command_name: &String,
        shell: &mut Shell,
        argv: &[String],
        io_ctx: CommandIoCtx,
    ) -> Result<i32, ExecutorError> {
        // 创建一个用于记录错误的stderr_writer
        let mut logger = io_ctx.stderr_writer()?;

        let result =
            Self::build_external_process(executable, command_name, shell, argv, io_ctx).status();

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

    fn build_external_process(
        executable: &Path,
        command_name: &String,
        shell: &Shell,
        argv: &[String],
        io_ctx: CommandIoCtx,
    ) -> ProcessCommand {
        let mut process = ProcessCommand::new(executable);
        process
            .arg0(command_name)
            .args(argv)
            .env_clear()
            .envs(shell.environment())
            .current_dir(shell.current_dir())
            .stdin(io_ctx.stdin)
            .stdout(io_ctx.stdout)
            .stderr(io_ctx.stderr);
        process
    }

    fn spawn_builtin_process(command_name: &str, argv: &[String], shell: &Shell, io_ctx: CommandIoCtx) -> io::Result<Child> {
        let executable = env::current_exe()?;

        ProcessCommand::new(executable)
            .arg0(builtin::BUILTIN_CHILD_ARG0)
            .arg(command_name)
            .args(argv)
            .env_clear()
            .envs(shell.environment())
            .stdin(io_ctx.stdin)
            .stdout(io_ctx.stdout)
            .stderr(io_ctx.stderr)
            .spawn()
    }

    fn execute_pipeline(
        &self,
        shell: &mut Shell,
        commands: Vec<Command>,
    ) -> Result<i32, ExecutorError> {
        let mut jobs = Vec::new();
        let mut previous_output: Option<PipeReader> = None;
        let commands_len = commands.len();
        for (idx, command) in commands.into_iter().enumerate() {
            let command = self.expand_command(command, shell)?;
            let mut io_ctx = CommandIoCtx::inherit()?;

            if let Some(previous_output) = previous_output.take() {
                io_ctx.replace_fd_with(0, File::from(OwnedFd::from(previous_output)))?;
            };

            let is_last_command = idx + 1 == commands_len;
            if !is_last_command{
                // 创建管道
                let (reader, writer) = io::pipe()?;
                io_ctx.replace_fd_with(1, File::from(OwnedFd::from(writer)))?;
                previous_output = Some(reader);
            }
            
            // 处理重定向，重定向优先级高于管道
            let io_ctx = io_ctx.apply_redirections(shell.current_dir(), &command.redirections)?;

            let Some((command_name, argv)) = command.args.split_first() else {
                // 空指令
                jobs.push(PipelineJob::Completed(0));
                continue;
            };

            match shell.resolve_command(command_name) {
                Some(ResolvedCommand::Builtin(_)) => {
                    let mut logger = io_ctx.stderr_writer()?;

                    match Self::spawn_builtin_process(command_name, argv, shell, io_ctx){
                        Ok(child) => {
                            jobs.push(PipelineJob::Process(child))
                        },
                        Err(error) => {
                            writeln!(logger, "{command_name}: failed to execute builtin: {error}")?;
                            jobs.push(PipelineJob::Completed(126));
                        }
                    }
                }
                Some(ResolvedCommand::External(executable)) => {
                    let mut logger = io_ctx.stderr_writer()?;
                    let handler = Self::build_external_process(
                        &executable,
                        command_name,
                        shell,
                        argv,
                        io_ctx,
                    )
                    .spawn();
                    match handler {
                        Ok(child) => {
                            jobs.push(PipelineJob::Process(child));
                        },
                        Err(error) => {
                            writeln!(logger, "{error}")?;
                            jobs.push(PipelineJob::Completed(126));
                        }
                    }
                    
                },
                None => {
                    // Command Not Found
                    writeln!(io_ctx.stderr_writer()?, "{}: not found", command_name)?;
                    jobs.push(PipelineJob::Completed(127));
                }
            }
        }

        let last_index = jobs.len() - 1;
        let mut last_status = 0;
        let mut first_error = None;

        for (idx, job) in jobs.into_iter().enumerate() {
            match job.wait() {
                Ok(status) if idx == last_index => {
                    last_status = status;
                }
                Ok(_) => {}
                Err(error) if first_error.is_none() => {
                    first_error = Some(error);
                }
                Err(_) => {}
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }

        Ok(last_status)
    }

    fn expand_command(
        &self,
        command: Command,
        shell: &mut Shell,
    ) -> Result<ExpandedCommand, ExecutorError> {
        Ok(Expander::new(shell.environment()).expand_command(command)?)
    }

    /// 处理external executable执行返回状态码的工具函数
    pub(crate) fn exit_status_code(status: ExitStatus) -> i32 {
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
