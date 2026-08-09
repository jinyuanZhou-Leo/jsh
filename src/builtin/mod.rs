mod cd;
mod echo;
mod exit;
mod pwd;
mod type_command;

pub(crate) use cd::cd;
pub(crate) use echo::echo;
pub(crate) use exit::exit;
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

    pub(crate) fn stdin(&mut self) -> &mut (dyn Read + '_) {
        self.stdin
    }

    pub(crate) fn stdout(&mut self) -> &mut (dyn Write + '_) {
        self.stdout
    }

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
    pub(crate) fn new(status: i32, message: impl Into<String>) -> Self {
        Self::Failure {
            status,
            message: message.into(),
        }
    }

    pub(crate) fn status(&self) -> i32 {
        match self {
            Self::Failure { status, .. } => *status,
            Self::Io(_) => 1,
        }
    }
}

pub(crate) const BUILTINS: [(&str, BuiltinFn); 5] = [
    ("exit", exit as BuiltinFn),
    ("echo", echo as BuiltinFn),
    ("type", type_command as BuiltinFn),
    ("pwd", pwd as BuiltinFn),
    ("cd", cd as BuiltinFn),
];

pub(crate) const BUILTIN_CHILD_ARG0: &str = "__jsh_builtin_child_mode__";

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

            // Builtin diagnostics must follow its stderr redirection.
            writeln!(io.stderr(), "{error}")?;

            Ok(status)
        }
    }
}