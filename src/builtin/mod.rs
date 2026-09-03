mod bg;
mod cd;
mod echo;
mod exit;
mod fg;
mod jobs;
mod pwd;
mod type_command;

pub(crate) use bg::bg;
pub(crate) use cd::cd;
pub(crate) use echo::echo;
pub(crate) use exit::exit;
pub(crate) use fg::fg;
pub(crate) use jobs::jobs;
pub(crate) use pwd::pwd;
pub(crate) use type_command::type_command;

use crate::shell::Shell;
use std::io::{self, Read, Write};
use thiserror::Error;

pub type BuiltinFn =
    for<'io> fn(shell: &mut Shell, args: &[String], io: &mut BuiltinIo<'io>) -> BuiltinOutput;
pub type BuiltinOutput = Result<i32, BuiltinError>;

pub(crate) struct BuiltinIo<'io> {
    stdin: &'io mut dyn Read,
    stdout: &'io mut dyn Write,
    stderr: &'io mut dyn Write,
}

impl<'io> BuiltinIo<'io> {
    /// 使用已准备的标准流创建内建命令 I/O 上下文。
    ///
    /// # Arguments
    ///
    /// * `stdin` - 内建命令的标准输入。
    /// * `stdout` - 内建命令的标准输出。
    /// * `stderr` - 内建命令的标准错误。
    ///
    /// # Returns
    ///
    /// 借用三个标准流的 [`BuiltinIo`]。
    pub(crate) fn new(
        stdin: &'io mut dyn Read,
        stdout: &'io mut dyn Write,
        stderr: &'io mut dyn Write,
    ) -> Self {
        Self {
            stdin,
            stdout,
            stderr,
        }
    }

    /// 返回内建命令标准输入的可变借用。
    ///
    /// # Returns
    ///
    /// 当前 I/O 上下文中的标准输入。
    pub(crate) fn stdin(&mut self) -> &mut (dyn Read + '_) {
        self.stdin
    }

    /// 返回内建命令标准输出的可变借用。
    ///
    /// # Returns
    ///
    /// 当前 I/O 上下文中的标准输出。
    pub(crate) fn stdout(&mut self) -> &mut (dyn Write + '_) {
        self.stdout
    }

    /// 返回内建命令标准错误的可变借用。
    ///
    /// # Returns
    ///
    /// 当前 I/O 上下文中的标准错误。
    pub(crate) fn stderr(&mut self) -> &mut (dyn Write + '_) {
        self.stderr
    }
}

#[derive(Debug, Error)]
pub(crate) enum BuiltinError {
    #[error("{message}")]
    Failure { status: i32, message: String },

    #[error("builtin I/O error: {0}")]
    Io(#[from] io::Error),
}

impl BuiltinError {
    /// 创建带状态码和诊断信息的内建命令失败。
    ///
    /// # Arguments
    ///
    /// * `status` - 内建命令失败时返回的状态码。
    /// * `message` - 写入标准错误的诊断信息。
    ///
    /// # Returns
    ///
    /// [`BuiltinError::Failure`] 错误值。
    pub(crate) fn new(status: i32, message: impl Into<String>) -> Self {
        Self::Failure {
            status,
            message: message.into(),
        }
    }

    /// 返回内建命令错误对应的 Shell 状态码。
    ///
    /// # Returns
    ///
    /// 业务失败返回其自带状态码，I/O 错误返回 1。
    pub(crate) fn status(&self) -> i32 {
        match self {
            Self::Failure { status, .. } => *status,
            Self::Io(_) => 1,
        }
    }
}

pub(crate) const BUILTINS: [(&str, BuiltinFn); 8] = [
    ("exit", exit as BuiltinFn),
    ("echo", echo as BuiltinFn),
    ("type", type_command as BuiltinFn),
    ("pwd", pwd as BuiltinFn),
    ("cd", cd as BuiltinFn),
    ("jobs", jobs as BuiltinFn),
    ("fg", fg as BuiltinFn),
    ("bg", bg as BuiltinFn),
];

/// 调用内建命令，并将业务错误写入该命令重定向后的标准错误。
///
/// # Arguments
///
/// * `builtin` - 待调用的内建命令函数。
/// * `shell` - 允许内建命令读取或修改的 Shell 上下文。
/// * `argv` - 不包含命令名的参数列表。
/// * `io` - 已应用重定向的内建命令 I/O。
///
/// # Returns
///
/// 内建命令成功或业务失败时返回其 Shell 状态码。
///
/// # Errors
///
/// 业务错误的诊断信息无法写入标准错误时返回 [`io::Error`]。
pub(crate) fn invoke(
    builtin: BuiltinFn,
    shell: &mut Shell,
    argv: &[String],
    io: &mut BuiltinIo<'_>,
) -> io::Result<i32> {
    match builtin(shell, argv, io) {
        Ok(status) => Ok(status),
        Err(error) => {
            let status = error.status();

            // 内建命令诊断信息必须遵守该命令的标准错误重定向。
            writeln!(io.stderr(), "{error}")?;

            Ok(status)
        }
    }
}
