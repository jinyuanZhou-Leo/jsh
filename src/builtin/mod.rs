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
use thiserror::Error;
use std::io::{self,Read, Write};

pub type BuiltinFn = for<'io > fn(shell: &mut Shell, args: &[String], io: & mut BuiltinIo<'io>) -> BuiltinOutput;
pub type BuiltinOutput = Result<i32, BuiltinError>;

pub(crate) struct BuiltinIo<'io>{
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
    Failure {
        status: i32,
        message: String,
    },

    #[error("builtin I/O error: {0}")]
    Io(#[from] io::Error),
}

impl BuiltinError {
    pub(crate) fn new(
        status: i32,
        message: impl Into<String>,
    ) -> Self {
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