use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, PipeReader, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use nix::{
    sys::signal::{Signal, kill, killpg},
    sys::wait::waitpid,
    unistd::{ForkResult, Pid, fork},
};

use crate::builtin::{self, BuiltinFn, BuiltinIo};
use crate::expander::{
    ExpandedCommand, ExpandedRedirectOperand, ExpandedRedirection, Expander, ExpanderError,
};
use crate::job_control::{JobControl, JobControlError, JobStage};
use crate::parser::Command;
use crate::shell::ResolvedCommand;
use crate::token::RedirectOperator;
use crate::{parser::Ast, shell::Shell};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fd(i32);

impl Fd {
    const STDIN: Self = Self(0);
    const STDOUT: Self = Self(1);
    const STDERR: Self = Self(2);
}

impl From<Fd> for i32 {
    fn from(fd: Fd) -> Self {
        fd.0
    }
}

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
    fn try_clone_fd(&self, fd: impl Into<i32>) -> Result<File, ExecutorError> {
        let fd = fd.into();
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
    fn replace_fd_with(
        &mut self,
        fd: impl Into<i32>,
        replacement: File,
    ) -> Result<(), ExecutorError> {
        let fd = fd.into();
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
                    self.replace_fd_with(Fd::STDIN, Self::open_input(&path)?)?;
                }
                (1, RedirectOperator::OutputTruncate, ExpandedRedirectOperand::Path(path)) => {
                    let path = Self::resolve_redirection_path(current_dir, path);
                    self.replace_fd_with(Fd::STDOUT, Self::open_output_truncate(&path)?)?;
                }
                (2, RedirectOperator::OutputTruncate, ExpandedRedirectOperand::Path(path)) => {
                    let path = Self::resolve_redirection_path(current_dir, path);
                    self.replace_fd_with(Fd::STDERR, Self::open_output_truncate(&path)?)?;
                }
                (1, RedirectOperator::OutputAppend, ExpandedRedirectOperand::Path(path)) => {
                    let path = Self::resolve_redirection_path(current_dir, path);
                    self.replace_fd_with(Fd::STDOUT, Self::open_output_append(&path)?)?;
                }
                (2, RedirectOperator::OutputAppend, ExpandedRedirectOperand::Path(path)) => {
                    let path = Self::resolve_redirection_path(current_dir, path);
                    self.replace_fd_with(Fd::STDERR, Self::open_output_append(&path)?)?;
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
    #[inline]
    fn resolve_redirection_path(current_dir: &Path, redirection_path: &str) -> PathBuf {
        current_dir.join(redirection_path)
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

enum Stage<T> {
    Ready(T),
    Finished(i32),
}

struct Prepared {
    command_name: String,
    argv: Vec<String>,
    io_ctx: CommandIoCtx,
}

type PreparedPipeline = Vec<Stage<Prepared>>;
type RunningPipeline = Vec<JobStage>;

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
        redirected_fd: i32,
        operator: RedirectOperator,
    },
    #[error("bad file descriptor: {fd}")]
    BadFileDescripter { fd: i32 },
    #[error("unsupported file descriptor: {fd}")]
    UnsupportedFileDescriptor { fd: i32 },
    #[error(transparent)]
    JobControl(#[from] JobControlError),
    #[error("failed to fork background job: {0}")]
    ForkBackground(nix::errno::Errno),
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
        let display_text = ast.to_string();
        self.execute_ast_with_display_text(shell, ast, &display_text)
    }

    /// 使用原始输入作为作业列表中的可读命令文本执行 AST。
    ///
    /// # Arguments
    ///
    /// * `shell` - 保存当前会话状态和 JobTable 的 Shell 上下文。
    /// * `ast` - 待执行的抽象语法树。
    /// * `source` - 用于 `jobs` 与状态通知的原始输入文本。
    ///
    /// # Returns
    ///
    /// AST 执行完成或后台作业成功登记后的 Shell 状态码。
    ///
    /// # Errors
    ///
    /// 命令展开、I/O 准备、进程启动或 Job Control 操作失败时返回错误。
    pub(crate) fn execute_with_source(
        &mut self,
        shell: &mut Shell,
        ast: Ast,
        source: &str,
    ) -> Result<i32, ExecutorError> {
        self.execute_ast_with_display_text(shell, ast, source)
    }

    /// 根据 AST 节点类型执行命令、条件列表或管道。
    ///
    /// # Arguments
    ///
    /// * `shell` - 保存当前会话状态的 Shell 上下文。
    /// * `ast` - 当前待执行的 AST 节点。
    /// * `display_text` - 登记运行期 Job 时使用的稳定展示文本。
    ///
    /// # Returns
    ///
    /// 当前节点执行后的 Shell 状态码。
    ///
    /// # Errors
    ///
    /// 子节点执行失败时返回 [`ExecutorError`]。
    fn execute_ast_with_display_text(
        &mut self,
        shell: &mut Shell,
        ast: Ast,
        display_text: &str,
    ) -> Result<i32, ExecutorError> {
        match ast {
            Ast::Command(command) => self.execute_command(shell, command, display_text),
            Ast::AndIf { left, right } => {
                let status = self.execute_ast_with_display_text(shell, *left, display_text)?;

                // 左侧成功且 Shell 未请求退出时，才执行右侧节点。
                if status == 0 && !shell.exit_requested() {
                    self.execute_ast_with_display_text(shell, *right, display_text)
                } else {
                    Ok(status)
                }
            }
            Ast::OrIf { left, right } => {
                let status = self.execute_ast_with_display_text(shell, *left, display_text)?;

                // 左侧失败且 Shell 未请求退出时，才执行右侧节点。
                if status != 0 && !shell.exit_requested() {
                    self.execute_ast_with_display_text(shell, *right, display_text)
                } else {
                    Ok(status)
                }
            }
            Ast::Pipeline { commands } => self.execute_pipeline(shell, commands, display_text),
            Ast::Seq(sequence) => {
                let mut last_status = 0;
                for item in sequence {
                    let item_display_text = item.to_string();
                    match self.execute_ast_with_display_text(shell, item, &item_display_text) {
                        Ok(status) => last_status = status,
                        Err(error) => {
                            eprintln!("Error occurred while executing sequence: `{error}`");
                            last_status = 3
                        }
                    }

                    if shell.exit_requested() {
                        break;
                    }
                }

                Ok(last_status)
            }
            Ast::Background { job } => self.execute_background(shell, *job, display_text),
        }
    }

    /// 展开并执行单条命令，包括重定向、命令查找和具体分派。
    ///
    /// # Arguments
    ///
    /// * `shell` - 提供环境变量、当前目录和命令解析能力的 Shell 上下文。
    /// * `command` - 尚未展开的命令。
    /// * `display_text` - 外部命令成为 Job 时使用的展示文本。
    ///
    /// # Returns
    ///
    /// 命令状态码；未找到命令时返回 127，空命令返回 0。
    ///
    /// # Errors
    ///
    /// 命令展开、重定向准备或诊断信息写入失败时返回 [`ExecutorError`]。
    fn execute_command(
        &mut self,
        shell: &mut Shell,
        command: Command,
        display_text: &str,
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
                self.execute_external(&path, command_name, shell, argv, io_ctx, display_text)
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
    /// # Returns
    ///
    /// 内建命令自身返回的 Shell 状态码。
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
    /// * `command_text` - 交互模式下登记前台 Job 时使用的展示文本。
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
        command_text: &str,
    ) -> Result<i32, ExecutorError> {
        // io_ctx 会被子进程接管，因此提前复制 stderr 用于报告启动错误。
        let mut logger = io_ctx.stderr_writer()?;

        let mut process =
            Self::build_external_process(executable, command_name, shell, argv, io_ctx);
        let target_pgid = shell.job_control().child_group_target(None);
        shell.job_control().configure_child_command(
            &mut process,
            target_pgid,
            shell.job_control().is_interactive(),
        );

        match process.spawn() {
            Ok(child) => {
                let pid = Pid::from_raw(child.id() as i32);
                drop(child);
                let pgid = match shell
                    .job_control()
                    .confirm_child_process_group(pid, target_pgid)
                {
                    Ok(pgid) => pgid,
                    Err(error) => {
                        let _ = kill(pid, Signal::SIGTERM);
                        let _ = waitpid(pid, None);
                        shell.job_control().restore_shell_terminal();
                        return Err(error.into());
                    }
                };
                let stages = vec![JobStage::Process(pid)];

                if shell.job_control().is_interactive() {
                    let job_id = shell.job_control_mut().register_job(
                        pgid,
                        command_text.to_owned(),
                        stages,
                    );
                    Ok(shell
                        .job_control_mut()
                        .wait_for_foreground_job(job_id, false)?)
                } else {
                    let mut stages = stages;
                    Ok(JobControl::wait_for_stages(&mut stages)?)
                }
            }
            Err(error) => {
                shell.job_control().restore_shell_terminal();
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

    /// 构造以当前程序的内建命令子进程模式运行的管道阶段。
    ///
    /// # Arguments
    ///
    /// * `command_name` - 需要执行的内建命令名。
    /// * `argv` - 不包含命令名的参数列表。
    /// * `shell` - 提供子进程环境变量的 Shell 上下文。
    /// * `io_ctx` - 移交给子进程的标准输入、标准输出和标准错误。
    ///
    /// # Returns
    ///
    /// 已配置参数、环境、当前目录和标准流，但尚未启动的 [`ProcessCommand`]。
    ///
    /// # Errors
    ///
    /// 无法定位当前可执行文件时返回 [`io::Error`]。
    fn build_builtin_process(
        command_name: &str,
        argv: &[String],
        shell: &Shell,
        io_ctx: CommandIoCtx,
    ) -> io::Result<ProcessCommand> {
        let executable = env::current_exe()?;

        let mut process = ProcessCommand::new(executable);
        process
            .arg0(builtin::BUILTIN_CHILD_ARG0)
            .arg(command_name)
            .args(argv)
            .env_clear()
            .envs(shell.environment())
            .current_dir(shell.current_dir())
            .stdin(io_ctx.stdin)
            .stdout(io_ctx.stdout)
            .stderr(io_ctx.stderr);
        Ok(process)
    }

    /// 在启动任何进程前完成 pipeline 的展开、pipe 拓扑和重定向准备。
    ///
    /// # Arguments
    ///
    /// * `shell` - 提供环境变量、当前目录和展开上下文的 Shell。
    /// * `commands` - 按 pipeline 顺序排列的未展开命令。
    ///
    /// # Returns
    ///
    /// 所有 I/O 和参数均已准备完成的阶段列表；空命令阶段表示为状态码 0。
    ///
    /// # Errors
    ///
    /// 展开、pipe 创建、文件描述符复制或重定向准备失败时返回错误；返回错误前不会启动子进程。
    fn prepare_pipeline(
        &self,
        shell: &mut Shell,
        commands: Vec<Command>,
    ) -> Result<PreparedPipeline, ExecutorError> {
        let mut prepared_pipeline = Vec::new();
        // 管道上一个job的输出pipe_reader
        let mut previous_output: Option<PipeReader> = None;
        let commands_len = commands.len();

        // 建立管道并处理重定向
        for (idx, command) in commands.into_iter().enumerate() {
            let command = self.expand_command(command, shell)?;
            let mut io_ctx = CommandIoCtx::inherit()?;

            if let Some(previous_output) = previous_output.take() {
                io_ctx.replace_fd_with(Fd::STDIN, File::from(OwnedFd::from(previous_output)))?;
            };

            let is_last_command = idx + 1 == commands_len;
            if !is_last_command {
                let (reader, writer) = io::pipe()?;
                io_ctx.replace_fd_with(Fd::STDOUT, File::from(OwnedFd::from(writer)))?;
                previous_output = Some(reader);
            }

            // 命令自身的重定向在管道端点之后应用，因此拥有更高优先级。
            let io_ctx = io_ctx.apply_redirections(shell.current_dir(), &command.redirections)?;

            // 不使用split_first, 减少一次堆分配
            let mut args = command.args;
            if args.is_empty() {
                prepared_pipeline.push(Stage::Finished(0));
                continue;
            }

            let command_name = args.remove(0);
            prepared_pipeline.push(Stage::Ready(Prepared {
                command_name,
                argv: args,
                io_ctx,
            })); // args移除第一个项目之后，变成argv
        }
        Ok(prepared_pipeline)
    }

    /// 启动已经完整准备的 pipeline，并把所有进程加入同一作业进程组。
    ///
    /// # Arguments
    ///
    /// * `shell` - 提供命令解析和 Job Control 配置的 Shell。
    /// * `prepared_pipeline` - 已完成所有可失败资源准备的 pipeline 阶段。
    ///
    /// # Returns
    ///
    /// 按原顺序保存 PID 或预先完成状态的运行期阶段。
    ///
    /// # Errors
    ///
    /// 某阶段发生内部启动错误时，先终止并回收此前启动的阶段，再返回该错误。
    fn spawn_pipeline(
        &self,
        shell: &mut Shell,
        prepared_pipeline: PreparedPipeline,
    ) -> Result<RunningPipeline, ExecutorError> {
        let mut running_pipeline = Vec::with_capacity(prepared_pipeline.len());
        let mut pgid = None;

        for job in prepared_pipeline {
            running_pipeline.push(match job {
                Stage::Ready(job) => match self.spawn_prepared_job(shell, job, pgid) {
                    // https://github.com/jinyuanZhou-Leo/jsh/issues/1
                    // prepared_job 生成失败时终止并回收已经启动的所有阶段。
                    Ok(stage) => {
                        if let JobStage::Process(pid) = stage
                            && pgid.is_none()
                        {
                            pgid = Some(shell.job_control().child_group_target(None).map_or(
                                pid,
                                |target| {
                                    if target.as_raw() == 0 { pid } else { target }
                                },
                            ));
                        }
                        stage
                    }
                    Err(error) => {
                        Self::terminate_pipeline(&running_pipeline, pgid);
                        return Err(error);
                    }
                },
                Stage::Finished(code) => JobStage::Completed(code),
            });
        }

        Ok(running_pipeline)
    }

    /// 解析并启动一个已经准备好的 pipeline 阶段。
    ///
    /// # Arguments
    ///
    /// * `shell` - 提供 builtin/external 解析和 Job Control 设置的 Shell。
    /// * `prepared_job` - 已展开参数并持有完整 I/O 的阶段。
    /// * `current_pgid` - pipeline 已建立的 PGID；首个可运行阶段传入 `None`。
    ///
    /// # Returns
    ///
    /// 启动成功时返回 PID；命令不存在或无法执行时返回 127/126 的完成阶段。
    ///
    /// # Errors
    ///
    /// 诊断流复制/写入或父进程侧进程组确认失败时返回错误；已启动但无法登记的子进程会先被回收。
    fn spawn_prepared_job(
        &self,
        shell: &Shell,
        prepared_job: Prepared,
        current_pgid: Option<Pid>,
    ) -> Result<JobStage, ExecutorError> {
        let Prepared {
            command_name,
            argv,
            io_ctx,
        } = prepared_job;

        let (mut process, mut logger) = match shell.resolve_command(&command_name) {
            Some(ResolvedCommand::Builtin(_)) => {
                let mut logger = io_ctx.stderr_writer()?;
                match Self::build_builtin_process(&command_name, &argv, shell, io_ctx) {
                    Ok(process) => (process, logger),
                    Err(error) => {
                        writeln!(logger, "{command_name}: failed to execute builtin: {error}",)?;
                        return Ok(JobStage::Completed(126));
                    }
                }
            }
            Some(ResolvedCommand::External(executable)) => {
                let logger = io_ctx.stderr_writer()?;
                (
                    Self::build_external_process(&executable, &command_name, shell, &argv, io_ctx),
                    logger,
                )
            }
            None => {
                writeln!(io_ctx.stderr_writer()?, "{command_name}: not found",)?;
                return Ok(JobStage::Completed(127));
            }
        };

        let target_pgid = shell.job_control().child_group_target(current_pgid);
        shell.job_control().configure_child_command(
            &mut process,
            target_pgid,
            shell.job_control().is_interactive(),
        );
        match process.spawn() {
            Ok(child) => {
                let pid = Pid::from_raw(child.id() as i32);
                drop(child);
                if let Err(error) = shell
                    .job_control()
                    .confirm_child_process_group(pid, target_pgid)
                {
                    let _ = kill(pid, Signal::SIGTERM);
                    let _ = waitpid(pid, None);
                    shell.job_control().restore_shell_terminal();
                    return Err(error.into());
                }
                Ok(JobStage::Process(pid))
            }
            Err(error) => {
                shell.job_control().restore_shell_terminal();
                writeln!(logger, "{command_name}: failed to execute: {error}")?;
                Ok(JobStage::Completed(126))
            }
        }
    }

    /// 在 pipeline 启动中途失败时终止并同步回收已经启动的阶段。
    ///
    /// # Arguments
    ///
    /// * `stages` - 当前已经发布到局部运行列表的阶段。
    /// * `pgid` - 已建立时用于一次性终止整个进程组；否则逐 PID 终止。
    ///
    /// 清理错误被忽略，以保留触发清理的原始错误。
    fn terminate_pipeline(stages: &[JobStage], pgid: Option<Pid>) {
        if let Some(pgid) = pgid {
            let _ = killpg(pgid, Signal::SIGTERM);
        } else {
            for stage in stages {
                if let JobStage::Process(pid) = stage {
                    let _ = kill(*pid, Signal::SIGTERM);
                }
            }
        }
        for stage in stages {
            if let JobStage::Process(pid) = stage {
                let _ = waitpid(*pid, None);
            }
        }
    }

    /// 启动管道的全部阶段，等待它们结束，并返回最后阶段的状态码。
    ///
    /// # Arguments
    ///
    /// * `shell` - 提供命令展开、解析、环境变量和当前目录的 Shell 上下文。
    /// * `commands` - 按执行顺序排列的管道命令。
    /// * `command_text` - 交互模式下登记 Job 时使用的展示文本。
    ///
    /// # Returns
    ///
    /// 管道最后一个阶段的 Shell 状态码。
    ///
    /// # Errors
    ///
    /// 管道创建、命令展开、重定向准备、进程启动诊断写入或进程等待失败时返回错误。
    fn execute_pipeline(
        &self,
        shell: &mut Shell,
        commands: Vec<Command>,
        command_text: &str,
    ) -> Result<i32, ExecutorError> {
        let prepared_pipeline = self.prepare_pipeline(shell, commands)?;
        let mut running_pipeline = self.spawn_pipeline(shell, prepared_pipeline)?;

        let status = if shell.job_control().is_interactive() {
            if let Some(pgid) = running_pipeline.iter().find_map(|stage| match stage {
                JobStage::Process(pid) => Some(*pid),
                JobStage::Completed(_) => None,
            }) {
                let job_id = shell.job_control_mut().register_job(
                    pgid,
                    command_text.to_owned(),
                    running_pipeline,
                );
                shell
                    .job_control_mut()
                    .wait_for_foreground_job(job_id, false)?
            } else {
                JobControl::wait_for_stages(&mut running_pipeline)?
            }
        } else {
            JobControl::wait_for_stages(&mut running_pipeline)?
        };

        Ok(status)
    }

    /// fork 后台监督进程、建立独立进程组，并把作业登记到父 Shell。
    ///
    /// # Arguments
    ///
    /// * `shell` - 父 Shell；其状态不会被后台 AST 中的 builtin 修改。
    /// * `ast` - 需要异步执行的完整命令、pipeline 或 AND-OR list。
    /// * `command_text` - JobTable 和启动通知使用的原始展示文本。
    ///
    /// # Returns
    ///
    /// 父进程成功登记后台作业后返回 0；子进程不会从该函数返回。
    ///
    /// # Errors
    ///
    /// `fork` 或父进程侧进程组确认失败时返回错误；无法确认进程组的子进程会先被终止并回收。
    fn execute_background(
        &mut self,
        shell: &mut Shell,
        ast: Ast,
        command_text: &str,
    ) -> Result<i32, ExecutorError> {
        // SAFETY: jsh does not create application threads. The child immediately resets its
        // signal state, isolates mutable Shell state, executes the AST, and exits via _exit.
        match unsafe { fork() }.map_err(ExecutorError::ForkBackground)? {
            ForkResult::Parent { child } => {
                let pgid = match shell
                    .job_control()
                    .confirm_child_process_group(child, Some(child))
                {
                    Ok(pgid) => pgid,
                    Err(error) => {
                        let _ = kill(child, Signal::SIGTERM);
                        let _ = waitpid(child, None);
                        return Err(error.into());
                    }
                };
                let text = command_text.trim_end().trim_end_matches('&').trim_end();
                let job_id = shell.job_control_mut().register_job(
                    pgid,
                    text.to_owned(),
                    vec![JobStage::Process(child)],
                );
                println!("[{job_id}] {}", pgid.as_raw());
                Ok(0)
            }
            ForkResult::Child => {
                let child_pid = nix::unistd::getpid();
                let status = Self::run_background_child(shell, child_pid, ast);
                // SAFETY: this is the forked supervisor process; destructors must not run over
                // state copied from the parent process.
                unsafe { nix::libc::_exit(status) }
            }
        }
    }

    /// 在 fork 得到的监督进程中初始化隔离 Shell 并执行后台 AST。
    ///
    /// # Arguments
    ///
    /// * `shell` - fork 时复制的父 Shell，只用于建立隔离状态快照。
    /// * `pgid` - 后台作业的进程组 ID。
    /// * `ast` - 要在隔离 Shell 中执行的完整 AST。
    ///
    /// # Returns
    ///
    /// AST 的 Shell 状态码；初始化失败返回 1，执行器内部错误返回 3。
    fn run_background_child(shell: &Shell, pgid: Pid, ast: Ast) -> i32 {
        if let Err(error) = JobControl::prepare_subshell_process(pgid) {
            eprintln!("jsh: failed to initialize background job: {error}");
            return 1;
        }

        if shell.job_control().is_non_interactive()
            && let Err(error) = Self::redirect_stdin_to_dev_null()
        {
            eprintln!("jsh: failed to redirect background stdin: {error}");
            return 1;
        }

        let mut child_shell = shell.forked_subshell(pgid);
        let mut executor = Self::new();
        match executor.execute(&mut child_shell, ast) {
            Ok(status) => status,
            Err(error) => {
                eprintln!("jsh: background job failed: {error}");
                3
            }
        }
    }

    /// 将当前进程的标准输入替换为只读 `/dev/null`。
    ///
    /// # Errors
    ///
    /// 打开 `/dev/null` 或执行 `dup2` 失败时返回 I/O 错误。
    fn redirect_stdin_to_dev_null() -> io::Result<()> {
        use std::os::fd::AsRawFd;

        let dev_null = File::open("/dev/null")?;
        // SAFETY: both descriptors are valid and dup2 atomically replaces standard input.
        if unsafe { nix::libc::dup2(dev_null.as_raw_fd(), Fd::STDIN.0) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
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
}

#[cfg(test)]
mod tests;
