mod builtin;
mod executor;
mod expander;
mod external;
mod job_control;
mod lexer;
mod parser;
mod shell;
mod token;
use std::{env, path::PathBuf};

use rustyline::{Config, error::ReadlineError};
use thiserror::Error;

use crate::{
    builtin::BUILTINS,
    executor::{Executor, ExecutorError},
    job_control::JobControlError,
    lexer::{Lexer, LexerError},
    parser::{Parser, ParserError},
    shell::Shell,
};

/// 运行交互式 Shell。
///
/// # Errors
///
/// 交互模式初始化失败、无法取得当前目录或读取输入失败时返回 [`ReplError`]。
fn main() -> Result<(), ReplError> {
    run_repl()
}

/// 初始化 Shell 会话并持续读取、解析和执行用户输入。
///
/// # Errors
///
/// 无法取得当前目录、初始化 Job Control/行编辑器、回收后台状态或读取用户输入时返回 [`ReplError`]。
fn run_repl() -> Result<(), ReplError> {
    // 读取环境变量
    let env = env::vars().collect();

    let mut shell = Shell::new(
        env::current_dir().map_err(ReplError::CurrentDirectory)?,
        env,
        BUILTINS,
    );
    shell.initialize_job_control()?;

    let mut executor = Executor::new();

    let config = Config::builder()
        .max_history_size(1000)
        .map_err(ReplError::ConfigureEditor)?
        .history_ignore_space(true)
        .history_ignore_dups(true)
        .map_err(ReplError::ConfigureEditor)?
        .auto_add_history(true)
        .build();
    let mut rl =
        rustyline::DefaultEditor::with_config(config).map_err(|_| ReplError::InitializeEditor)?;

    let history_file_path = history_file_path(&shell);
    if let Some(path) = &history_file_path {
        match rl.load_history(path) {
            Ok(()) => {}
            Err(ReadlineError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!("jsh: failed to load history: {error}");
            }
        }
    }

    while !shell.exit_requested() {
        for notification in shell.job_control_mut().take_notifications()? {
            eprintln!("{notification}");
        }

        // 读取用户输入
        let source = match rl.readline("$ ") {
            Ok(source) => {
                if let Some(path) = &history_file_path
                    && let Err(error) = rl.append_history(path)
                {
                    eprintln!("jsh: failed to append history: {error}");
                }
                source
            }
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

    shell.shutdown_job_control();
    Ok(())
}

/// 对一行源文本依次执行词法分析、语法分析和命令执行。
///
/// # Arguments
///
/// * `shell` - 保存当前会话状态的 Shell 上下文。
/// * `executor` - 执行解析结果的命令执行器。
/// * `source` - 用户输入的原始命令文本。
///
/// # Returns
///
/// 命令的 Shell 状态码；空输入返回 0。
///
/// # Errors
///
/// 词法分析、语法分析或执行阶段失败时返回 [`EvalError`]。
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

    Ok(executor.execute_with_source(shell, ast, source)?)
}

/// 解析当前 Shell 会话使用的历史记录文件路径。
///
/// # Arguments
///
/// * `shell` - 提供当前目录和 `HISTFILE` 环境变量的 Shell。
///
/// # Returns
///
/// `HISTFILE` 为空时返回 `None`；显式相对路径基于 Shell 当前目录解析；未设置时返回用户目录下的 `.jsh_history`。
fn history_file_path(shell: &Shell) -> Option<PathBuf> {
    match shell.env("HISTFILE") {
        Some("") => None,
        Some(path) => Some(shell.current_dir().join(path)),
        None => env::home_dir().map(|home_dir| home_dir.join(".jsh_history")),
    }
}

#[derive(Debug, Error)]
pub(crate) enum ReplError {
    #[error("failed to get current directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),

    #[error("failed to read line")]
    ReadLine(#[from] ReadlineError),

    #[error("failed to initialize editor")]
    InitializeEditor,

    #[error("failed to configure editor: {0}")]
    ConfigureEditor(#[source] ReadlineError),

    #[error("failed to save history: {0}")]
    SaveHistory(#[source] ReadlineError),

    #[error(transparent)]
    JobControl(#[from] JobControlError),
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
    /// 返回不同求值阶段发生错误时使用的 Shell 状态码。
    ///
    /// # Returns
    ///
    /// Lexer、Parser、Executor 错误分别对应 1、2、3。
    fn status(&self) -> i32 {
        match self {
            Self::Executor(_) => 3,
            Self::Parser(_) => 2,
            Self::Lexer(_) => 1,
        }
    }
}
