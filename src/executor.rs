use std::env::args;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtin::{BuiltinFn, BuiltinIo};
use crate::expander::{ExpandedRedirection, Expander, ExpanderError};
use crate::parser::Command;
use crate::shell::ResolvedCommand;
use crate::token::RedirectOperator;
use crate::{parser::Ast, shell::Shell};
use thiserror::Error;

/// Some表示覆盖默认的stdio, None表示继承终端
#[derive(Default, Debug)]
struct PreparedIo {
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

impl PreparedIo {
    fn stderr_writer(&self) -> io::Result<Box<dyn Write>> {
        match &self.stderr {
            Some(file) => Ok(Box::new(file.try_clone()?)),
            None => Ok(Box::new(io::stderr())),
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

    #[error("unsupported redirection: fd {fd} with operator {operator:?}")]
    UnsupportedRedirection { fd: u32, operator: RedirectOperator },
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

    pub(crate) fn execute_builtin(
        &self,
        builtin: BuiltinFn,
        shell: &mut Shell,
        argv: &[String],
        mut prepared_io: PreparedIo,
    ) -> Result<i32, ExecutorError> {
        let stdin: &mut dyn Read = match prepared_io.stdin.as_mut() {
            Some(file) => file,
            None => &mut io::stdin(),
        };

        let stdout: &mut dyn Write = match prepared_io.stdout.as_mut() {
            Some(file) => file,
            None => &mut io::stdout(),
        };

        let stderr: &mut dyn Write = match prepared_io.stderr.as_mut() {
            Some(file) => file,
            None => &mut io::stderr(),
        };

        let mut io = BuiltinIo::new(stdin, stdout, stderr);

        match builtin(shell, argv, &mut io) {
            Ok(code) => Ok(code),
            Err(error) => {
                let status = error.status();

                // 此处同样遵守重定向
                writeln!(io.stderr(), "{error}");
                Ok(status)
            }
        }
    }

    pub(crate) fn execute_external(
        &self,
        executable: &Path,
        command_name: &String,
        shell: &mut Shell,
        argv: &[String],
        prepared_io: PreparedIo,
    ) -> Result<i32, ExecutorError> {
        // 创建一个用于记录错误的stderr_writer
        let mut logger = prepared_io.stderr_writer()?;

        let stdin: Stdio = match prepared_io.stdin {
            Some(file) => Stdio::from(file),
            None => Stdio::inherit(),
        };

        let stdout: Stdio = match prepared_io.stdout {
            Some(file) => Stdio::from(file),
            None => Stdio::inherit(),
        };

        let stderr: Stdio = match prepared_io.stderr {
            Some(file) => Stdio::from(file),
            None => Stdio::inherit(),
        };

        let result = ProcessCommand::new(executable)
            .arg0(command_name)
            .args(argv)
            .env_clear()
            .envs(shell.environment())
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
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
        let mut prepared_io = PreparedIo::default();

        // 按照顺序处理重定向
        // 靠后的重定向会覆盖前面的重定向
        for redirection in redirections {
            let path = self.resolve_redirection_target(current_dir, &redirection.target);

            match (redirection.fd, redirection.operator) {
                (0, RedirectOperator::Input) => {
                    prepared_io.stdin = Some(Self::open_input(&path)?);
                }
                (1, RedirectOperator::OutputTruncate) => {
                    prepared_io.stdout = Some(Self::open_output_truncate(&path)?);
                }
                (2, RedirectOperator::OutputTruncate) => {
                    prepared_io.stderr = Some(Self::open_output_truncate(&path)?);
                }
                (1, RedirectOperator::OutputAppend) => {
                    prepared_io.stdout = Some(Self::open_output_append(&path)?);
                }
                (2, RedirectOperator::OutputAppend) => {
                    prepared_io.stderr = Some(Self::open_output_append(&path)?);
                }
                (fd, operator) => {
                    return Err(ExecutorError::UnsupportedRedirection { fd, operator });
                }
            }
        }

        Ok(prepared_io)
    }

    /// 将redirection_target转换成绝对路径
    ///
    /// # Arguments
    ///
    /// - `current_dir` (`&Path`) - shell当前运行目录
    /// - `redirection_target` (`&str`) - 重定向目标
    ///
    /// # Returns
    ///
    /// - `PathBuf` - 重定向目标的绝对路径
    /// ```
    fn resolve_redirection_target(&self, current_dir: &Path, redirection_target: &str) -> PathBuf {
        let target = Path::new(redirection_target);

        if target.is_absolute() {
            PathBuf::from(target)
        } else {
            current_dir.join(target)
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
