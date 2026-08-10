use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, PipeReader, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, ExitStatus};

use crate::builtin::{self, BuiltinFn, BuiltinIo};
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
    /// 从当前进程复制标准输入、标准输出和标准错误的文件描述符。
    ///
    /// # Errors
    ///
    /// 任一标准文件描述符复制失败时返回 [`io::Error`]。
    fn inherit() -> io::Result<Self> {
        Ok(Self {
            stdin: File::from(io::stdin().as_fd().try_clone_to_owned()?),
            stdout: File::from(io::stdout().as_fd().try_clone_to_owned()?),
            stderr: File::from(io::stderr().as_fd().try_clone_to_owned()?),
        })
    }

    /// 复制指定的标准文件描述符。
    ///
    /// 目前执行器只支持标准输入、标准输出和标准错误，即文件描述符 0、1、2。
    ///
    /// # Arguments
    ///
    /// * `fd` - 需要复制的文件描述符。
    ///
    /// # Errors
    ///
    /// 当 `fd` 不属于 0、1、2，或底层文件描述符复制失败时返回错误。
    fn try_clone_fd(&self, fd: u32) -> Result<File, ExecutorError> {
        match fd {
            0 => Ok(self.stdin.try_clone()?),
            1 => Ok(self.stdout.try_clone()?),
            2 => Ok(self.stderr.try_clone()?),
            _ => Err(ExecutorError::BadFileDescripter { fd }),
        }
    }

    /// 用给定文件替换指定的标准文件描述符。
    ///
    /// # Arguments
    ///
    /// * `fd` - 需要替换的目标文件描述符。
    /// * `replacement` - 接管目标文件描述符的新文件。
    ///
    /// # Errors
    ///
    /// 当 `fd` 不属于 0、1、2 时返回 [`ExecutorError::UnsupportedFileDescriptor`]。
    fn replace_fd_with(&mut self, fd: u32, replacement: File) -> Result<(), ExecutorError> {
        match fd {
            0 => self.stdin = replacement,
            1 => self.stdout = replacement,
            2 => self.stderr = replacement,
            _ => return Err(ExecutorError::UnsupportedFileDescriptor { fd }),
        }

        Ok(())
    }

    /// 复制标准错误，供执行失败时写入诊断信息。
    ///
    /// # Errors
    ///
    /// 标准错误文件描述符复制失败时返回 [`io::Error`]。
    fn stderr_writer(&self) -> io::Result<File> {
        self.stderr.try_clone()
    }

    /// 按源码顺序应用重定向，使后出现的重定向覆盖先出现的重定向。
    ///
    /// # Arguments
    ///
    /// * `current_dir` - 解析相对重定向路径时使用的当前目录。
    /// * `redirections` - 已展开且保持源码顺序的重定向列表。
    ///
    /// # Errors
    ///
    /// 当文件打开、文件描述符复制失败，或遇到不支持的重定向时返回错误。
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

    /// 将相对重定向路径解析到 Shell 维护的当前目录下。
    ///
    /// # Arguments
    ///
    /// * `current_dir` - Shell 当前目录。
    /// * `redirection_path` - 重定向操作数中的原始路径。
    ///
    /// # Returns
    ///
    /// 绝对路径保持不变；相对路径拼接到 `current_dir` 后返回。
    fn resolve_redirection_path(current_dir: &Path, redirection_path: &str) -> PathBuf {
        let path = Path::new(redirection_path);

        if path.is_absolute() {
            path.to_path_buf()
        } else {
            current_dir.join(path)
        }
    }

    /// 以只读方式打开输入重定向目标。
    ///
    /// # Arguments
    ///
    /// * `path` - 输入重定向目标路径。
    ///
    /// # Errors
    ///
    /// 目标无法以只读方式打开时返回 [`ExecutorError::OpenRedirection`]。
    fn open_input(path: &Path) -> Result<File, ExecutorError> {
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|source| ExecutorError::OpenRedirection {
                path: path.to_path_buf(),
                source,
            })
    }

    /// 创建或截断输出重定向目标，并以写入方式打开。
    ///
    /// # Arguments
    ///
    /// * `path` - 输出重定向目标路径。
    ///
    /// # Errors
    ///
    /// 目标无法创建、截断或打开时返回 [`ExecutorError::OpenRedirection`]。
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

    /// 创建输出重定向目标（若不存在），并以追加方式打开。
    ///
    /// # Arguments
    ///
    /// * `path` - 输出重定向目标路径。
    ///
    /// # Errors
    ///
    /// 目标无法创建或以追加方式打开时返回 [`ExecutorError::OpenRedirection`]。
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
    /// 等待管道阶段结束，并将进程退出状态转换为 Shell 状态码。
    ///
    /// # Errors
    ///
    /// 等待子进程失败时返回 [`ExecutorError::WaitPipelineProcess`]。
    fn wait(self) -> Result<i32, ExecutorError> {
        match self {
            Self::Completed(status) => Ok(status),
            Self::Process(mut child) => child
                .wait()
                .map(Executor::exit_status_code)
                .map_err(ExecutorError::WaitPipelineProcess),
        }
    }
}

/// 负责展开并执行 Shell 抽象语法树。
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
    /// 创建一个无状态的执行器。
    ///
    /// # Returns
    ///
    /// 可用于执行 Shell 抽象语法树的 [`Executor`]。
    pub(crate) fn new() -> Self {
        Self {}
    }

    /// 执行一棵抽象语法树并返回 Shell 状态码。
    ///
    /// # Arguments
    ///
    /// * `shell` - 保存当前会话状态的 Shell 上下文。
    /// * `ast` - 待执行的抽象语法树。
    ///
    /// # Returns
    ///
    /// 命令、条件列表或管道的最终 Shell 状态码。
    ///
    /// # Errors
    ///
    /// 命令展开、I/O 准备或进程等待失败时返回 [`ExecutorError`]。
    pub(crate) fn execute(&mut self, shell: &mut Shell, ast: Ast) -> Result<i32, ExecutorError> {
        self.execute_ast(shell, ast)
    }

    /// 根据 AST 节点类型执行命令、条件列表或管道。
    ///
    /// # Arguments
    ///
    /// * `shell` - 保存当前会话状态的 Shell 上下文。
    /// * `ast` - 当前待执行的 AST 节点。
    ///
    /// # Returns
    ///
    /// 当前节点执行后的 Shell 状态码。
    ///
    /// # Errors
    ///
    /// 子节点执行失败时返回 [`ExecutorError`]。
    pub(crate) fn execute_ast(
        &mut self,
        shell: &mut Shell,
        ast: Ast,
    ) -> Result<i32, ExecutorError> {
        match ast {
            Ast::Command(command) => self.execute_command(shell, command),
            Ast::AndIf { left, right } => {
                let status = self.execute_ast(shell, *left)?;

                // 左侧成功且 Shell 未请求退出时，才执行右侧节点。
                if status == 0 && !shell.exit_requested() {
                    self.execute_ast(shell, *right)
                } else {
                    Ok(status)
                }
            }
            Ast::OrIf { left, right } => {
                let status = self.execute_ast(shell, *left)?;

                // 左侧失败且 Shell 未请求退出时，才执行右侧节点。
                if status != 0 && !shell.exit_requested() {
                    self.execute_ast(shell, *right)
                } else {
                    Ok(status)
                }
            }
            Ast::Pipeline { commands } => self.execute_pipeline(shell, commands),
        }
    }

    /// 展开并执行单条命令，包括重定向、命令查找和具体分派。
    ///
    /// # Arguments
    ///
    /// * `shell` - 提供环境变量、当前目录和命令解析能力的 Shell 上下文。
    /// * `command` - 尚未展开的命令。
    ///
    /// # Returns
    ///
    /// 命令状态码；未找到命令时返回 127，空命令返回 0。
    ///
    /// # Errors
    ///
    /// 命令展开、重定向准备或诊断信息写入失败时返回 [`ExecutorError`]。
    pub(crate) fn execute_command(
        &mut self,
        shell: &mut Shell,
        command: Command,
    ) -> Result<i32, ExecutorError> {
        let expanded_command = self.expand_command(command, shell)?;
        // 重定向必须先于命令查找：仅包含重定向的空命令也是合法命令。
        let io_ctx = CommandIoCtx::inherit()?
            .apply_redirections(shell.current_dir(), &expanded_command.redirections)?;

        let Some((command_name, argv)) = expanded_command.args.split_first() else {
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
                // 未找到命令不是执行器内部错误，按 Shell 约定返回 127。
                writeln!(io_ctx.stderr_writer()?, "{}: not found", command_name)?;
                Ok(127)
            }
        }
    }

    /// 在当前 Shell 上下文中同步执行内建命令。
    ///
    /// # Arguments
    ///
    /// * `builtin` - 待调用的内建命令函数。
    /// * `shell` - 允许内建命令读取或修改的 Shell 上下文。
    /// * `argv` - 不包含命令名的参数列表。
    /// * `io_ctx` - 已应用重定向的命令 I/O。
    ///
    /// # Errors
    ///
    /// 内建命令的诊断信息无法写入时返回 [`ExecutorError`]。
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

    /// 同步执行外部命令，并将启动失败转换为状态码 126。
    ///
    /// # Arguments
    ///
    /// * `executable` - 已解析的可执行文件路径。
    /// * `command_name` - 用户输入的命令名，用作子进程的 `argv[0]`。
    /// * `shell` - 提供环境变量和当前目录的 Shell 上下文。
    /// * `argv` - 不包含命令名的参数列表。
    /// * `io_ctx` - 已应用重定向的命令 I/O。
    ///
    /// # Returns
    ///
    /// 外部命令的 Shell 状态码；进程启动失败时返回 126。
    ///
    /// # Errors
    ///
    /// 标准错误复制或启动失败诊断信息写入失败时返回 [`ExecutorError`]。
    fn execute_external(
        &self,
        executable: &Path,
        command_name: &String,
        shell: &mut Shell,
        argv: &[String],
        io_ctx: CommandIoCtx,
    ) -> Result<i32, ExecutorError> {
        // io_ctx 会被子进程接管，因此提前复制 stderr 用于报告启动错误。
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

                // 命令已找到但无法执行，对应状态码 126。
                Ok(126)
            }
        }
    }

    /// 构造继承 Shell 环境、当前目录和已准备 I/O 的外部进程。
    ///
    /// # Arguments
    ///
    /// * `executable` - 需要执行的文件路径。
    /// * `command_name` - 设置为子进程 `argv[0]` 的命令名。
    /// * `shell` - 提供子进程环境变量和当前目录的 Shell 上下文。
    /// * `argv` - 不包含命令名的参数列表。
    /// * `io_ctx` - 移交给子进程的标准输入、标准输出和标准错误。
    ///
    /// # Returns
    ///
    /// 已完成参数和环境配置、但尚未启动的 [`ProcessCommand`]。
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

    /// 以当前程序的内建命令子进程模式启动一个管道阶段。
    ///
    /// # Arguments
    ///
    /// * `command_name` - 需要执行的内建命令名。
    /// * `argv` - 不包含命令名的参数列表。
    /// * `shell` - 提供子进程环境变量的 Shell 上下文。
    /// * `io_ctx` - 移交给子进程的标准输入、标准输出和标准错误。
    ///
    /// # Errors
    ///
    /// 无法定位当前可执行文件或无法启动子进程时返回 [`io::Error`]。
    fn spawn_builtin_process(
        command_name: &str,
        argv: &[String],
        shell: &Shell,
        io_ctx: CommandIoCtx,
    ) -> io::Result<Child> {
        let executable = env::current_exe()?;

        ProcessCommand::new(executable)
            .arg0(builtin::BUILTIN_CHILD_ARG0)
            .arg(command_name)
            .args(argv)
            .env_clear()
            .envs(shell.environment())
            .current_dir(shell.current_dir())
            .stdin(io_ctx.stdin)
            .stdout(io_ctx.stdout)
            .stderr(io_ctx.stderr)
            .spawn()
    }

    /// 启动管道的全部阶段，等待它们结束，并返回最后阶段的状态码。
    ///
    /// # Arguments
    ///
    /// * `shell` - 提供命令展开、解析、环境变量和当前目录的 Shell 上下文。
    /// * `commands` - 按执行顺序排列的管道命令。
    ///
    /// # Returns
    ///
    /// 管道最后一个阶段的 Shell 状态码。
    ///
    /// # Errors
    ///
    /// 管道创建、命令展开、重定向准备、进程启动诊断写入或进程等待失败时返回错误。
    ///
    /// # Panics
    ///
    /// `commands` 为空时会触发 panic。解析器生成的管道至少包含两个命令。
    fn execute_pipeline(
        &self,
        shell: &mut Shell,
        commands: Vec<Command>,
    ) -> Result<i32, ExecutorError> {
        let mut jobs = Vec::new();
        let mut previous_output: Option<PipeReader> = None;
        let commands_len = commands.len();

        // 先逐阶段建立管道并启动进程，避免在生产者运行前等待消费者。
        for (idx, command) in commands.into_iter().enumerate() {
            let command = self.expand_command(command, shell)?;
            let mut io_ctx = CommandIoCtx::inherit()?;

            if let Some(previous_output) = previous_output.take() {
                io_ctx.replace_fd_with(0, File::from(OwnedFd::from(previous_output)))?;
            };

            let is_last_command = idx + 1 == commands_len;
            if !is_last_command {
                let (reader, writer) = io::pipe()?;
                io_ctx.replace_fd_with(1, File::from(OwnedFd::from(writer)))?;
                previous_output = Some(reader);
            }

            // 命令自身的重定向在管道端点之后应用，因此拥有更高优先级。
            let io_ctx = io_ctx.apply_redirections(shell.current_dir(), &command.redirections)?;

            let Some((command_name, argv)) = command.args.split_first() else {
                jobs.push(PipelineJob::Completed(0));
                continue;
            };

            match shell.resolve_command(command_name) {
                Some(ResolvedCommand::Builtin(_)) => {
                    let mut logger = io_ctx.stderr_writer()?;

                    match Self::spawn_builtin_process(command_name, argv, shell, io_ctx) {
                        Ok(child) => jobs.push(PipelineJob::Process(child)),
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
                        }
                        Err(error) => {
                            writeln!(logger, "{error}")?;
                            jobs.push(PipelineJob::Completed(126));
                        }
                    }
                }
                None => {
                    writeln!(io_ctx.stderr_writer()?, "{}: not found", command_name)?;
                    jobs.push(PipelineJob::Completed(127));
                }
            }
        }

        let last_index = jobs.len() - 1;
        let mut last_status = 0;
        let mut first_error = None;

        // 即使某个 wait 失败，也继续回收其余子进程，最后再返回首个等待错误。
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

    /// 使用当前 Shell 环境展开命令参数和重定向操作数。
    ///
    /// # Arguments
    ///
    /// * `command` - 尚未展开的命令。
    /// * `shell` - 提供展开所需环境变量的 Shell 上下文。
    ///
    /// # Errors
    ///
    /// 参数或重定向操作数展开失败时返回 [`ExecutorError::Expansion`]。
    fn expand_command(
        &self,
        command: Command,
        shell: &mut Shell,
    ) -> Result<ExpandedCommand, ExecutorError> {
        Ok(Expander::new(shell.environment()).expand_command(command)?)
    }

    /// 将外部进程退出状态转换为 Shell 状态码。
    ///
    /// # Arguments
    ///
    /// * `status` - 外部进程的原始退出状态。
    ///
    /// # Returns
    ///
    /// 正常退出时返回进程状态码；Unix 信号终止时返回 `128 + signal`；无法取得上述信息时返回 1。
    pub(crate) fn exit_status_code(status: ExitStatus) -> i32 {
        if let Some(code) = status.code() {
            return code;
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            // Unix 进程被信号终止时，Shell 状态码约定为 128 + 信号编号。
            if let Some(signal) = status.signal() {
                return 128 + signal;
            }
        }

        1
    }
}

#[cfg(test)]
mod tests;
