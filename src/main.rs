mod builtin;
mod executor;
mod expander;
mod external;
mod lexer;
mod parser;
mod shell;
mod token;
use std::{
    collections::HashMap,
    env,
    io::{self, Write},
};

use thiserror::Error;

use crate::{
    executor::{Executor, ExecutorError},
    lexer::{Lexer, LexerError},
    parser::{Parser, ParserError},
    shell::Shell,
};

fn main() -> Result<(), ReplError> {
    // 读取环境变量
    let mut env = HashMap::new();
    for (k, v) in env::vars() {
        env.insert(k, v);
    }

    let mut shell = Shell::new(
        env::current_dir().map_err(ReplError::CurrentDirectory)?,
        env,
        [
            ("exit", builtin::exit),
            ("echo", builtin::echo),
            ("type", builtin::type_command),
            ("pwd", builtin::pwd),
            ("cd", builtin::cd),
        ],
    );

    let mut executor = Executor::new();

    while !shell.exit_requested() {
        print!("$ ");
        io::stdout().flush().map_err(ReplError::FlushPrompt)?;

        // 读取用户输入
        let mut source = String::new();
        match io::stdin().read_line(&mut source) {
            // 读入内容长度为 0 bytes
            Ok(0) => break,
            // 正常情况
            Ok(_) => {}
            Err(e) => return Err(ReplError::ReadLine(e)),
        };

        match execute_line(&mut shell, &mut executor, &source.trim_end()) {
            Ok(code) => {
                shell.set_last_status(code);
            },
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
    let lexer = Lexer::new();
    let tokens = lexer.lex(&source)?;

    let Some(ast) = Parser::new(tokens).parse()? else {
        // Ast为空，空输入返回0
        return Ok(0);
    };

    Ok(executor.execute(shell, ast)?)
}

#[derive(Debug, Error)]
pub(crate) enum ReplError {
    #[error("failed to flush prompt: {0}")]
    FlushPrompt(#[source] std::io::Error),
    #[error("failed to get current directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),
    #[error("failed to read line")]
    ReadLine(#[source] std::io::Error),
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
            Self::Lexer(_) => 1
        }
    }
}
