mod builtin;
mod executor;
mod expander;
mod external;
mod lexer;
mod parser;
mod shell;
mod token;
use std::env;

use rustyline::error::ReadlineError;
use thiserror::Error;

use crate::{
    builtin::{BUILTIN_CHILD_ARG0, BUILTINS, BuiltinIo}, executor::{Executor, ExecutorError}, lexer::{Lexer, LexerError}, parser::{Parser, ParserError}, shell::{ResolvedCommand, Shell},
};

fn main() -> Result<(), ReplError> {
    let mut args = env::args();

    if args.next().as_deref() == Some(BUILTIN_CHILD_ARG0) {
        // 内建指令子进程模式】
        std::process::exit(run_builtin_child(args));
    } else {
        run_repl()
    }
}

fn run_repl() -> Result<(), ReplError> {
    // 读取环境变量
    let env = env::vars().collect();

    let mut shell = Shell::new(
        env::current_dir().map_err(ReplError::CurrentDirectory)?,
        env,
        BUILTINS,
    );

    let mut executor = Executor::new();

    let mut editor = rustyline::DefaultEditor::new().map_err(|_| ReplError::InitializeEditor)?;

    while !shell.exit_requested() {
        // 读取用户输入
        let source = match editor.readline("$ ") {
            Ok(source) => source,
            Err(ReadlineError::Eof) => {
                // Ctrl/Cmd + D 退出终端
                shell.request_exit(0);
                continue;
            }
            Err(ReadlineError::Interrupted) => {
                // 输入被打断，进入下一次循环
                continue;
            }
            Err(error) => {
                return Err(ReplError::ReadLine(error));
            }
        };

        match execute_line(&mut shell, &mut executor, &source) {
            Ok(code) => {
                shell.set_last_status(code);
            }
            Err(error) => {
                shell.set_last_status(error.status());
                eprintln!("{error}");
            }
        }
    }

    Ok(())
}

fn execute_line(
    shell: &mut Shell,
    executor: &mut Executor,
    source: &str,
) -> Result<i32, EvalError> {
    let tokens = Lexer::new(source).lex()?;

    let Some(ast) = Parser::new(tokens).parse()? else {
        // Ast为空，空输入返回0
        return Ok(0);
    };

    Ok(executor.execute(shell, ast)?)
}

fn run_builtin_child(mut args: impl Iterator<Item = String>) -> i32 {
    let Some(command_name) = args.next() else {
        eprintln!("jsh: missing builtin name in child process");
        return 2;
    };

    let argv: Vec<_> = args.collect();

    let current_dir = match env::current_dir() {
        Ok(current_dir) => current_dir,
        Err(error) => {
            eprintln!("jsh: failed to get current directory: {error}");
            return 1;
        }
    };

    let mut shell = Shell::new(
        current_dir,
        env::vars().collect(),
        builtin::BUILTINS,
    );

    let builtin = match shell.resolve_command(&command_name) {
        Some(ResolvedCommand::Builtin(builtin)) => builtin,
        _ => {
            eprintln!("jsh: unknown builtin `{command_name}`");
            return 126;
        }
    };

    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();

    let mut io = BuiltinIo::new(
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );

    match builtin::invoke(
        builtin,
        &mut shell,
        &argv,
        &mut io,
    ) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("jsh: builtin I/O failed: {error}");
            1
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum ReplError {
    #[error("failed to flush prompt: {0}")]
    FlushPrompt(#[source] std::io::Error),
    #[error("failed to get current directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),
    #[error("failed to read line")]
    ReadLine(#[source] ReadlineError),
    #[error("failed to initialize editor")]
    InitializeEditor,
}

#[derive(Debug, Error)]
enum EvalError {
    #[error(transparent)]
    Lexer(#[from] LexerError),

    #[error(transparent)]
    Parser(#[from] ParserError),

    #[error(transparent)]
    Executor(#[from] ExecutorError),
}

impl EvalError {
    fn status(&self) -> i32 {
        match self {
            Self::Executor(_) => 3,
            Self::Parser(_) => 2,
            Self::Lexer(_) => 1,
        }
    }
}
