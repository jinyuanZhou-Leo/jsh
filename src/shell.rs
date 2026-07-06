use std::{
    collections::HashMap,
    error::Error,
    fmt,
    io::{self, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{self, Command},
};

use crate::{expender::Expander, external::CommandLoader, lexer::Lexer};

/// Shell: shell context
/// &[String]: command arguments
pub type BuiltinFn = fn(&mut Shell, &[String]) -> BuiltinOutput;
pub type BuiltinOutput = Result<BuiltinResult, BuiltinError>;

pub struct BuiltinResult {
    pub code: i32,
}

#[derive(Debug)]
pub struct BuiltinError {
    pub code: i32,
    pub message: String,
}

impl BuiltinError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for BuiltinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for BuiltinError {}

#[derive(Default)]
pub struct Shell {
    // Current working directory
    pwd: PathBuf,
    // Environment variables
    env_vars: HashMap<String, String>,
    // Builtin commands
    builtin: HashMap<String, BuiltinFn>,
    command_loader: CommandLoader,
    should_exit: bool,
    exit_code: i32,
    status: i32,
}

impl Shell {
    pub fn new<const N: usize>(
        pwd: impl Into<PathBuf>,
        env_vars: HashMap<String, String>,
        builtin: [(impl Into<String>, BuiltinFn); N],
    ) -> Self {
        let command_loader = CommandLoader::new(&env_vars);
        Self {
            pwd: pwd.into(),
            env_vars,
            builtin: builtin.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            command_loader: command_loader,
            should_exit: false,
            exit_code: 0,
            status: 0,
        }
    }
    pub fn pwd(&self) -> &Path {
        self.pwd.as_path()
    }

    pub fn set_pwd(&mut self, path: impl Into<PathBuf>) {
        self.pwd = path.into();
    }

    pub fn env_vars(&self) -> &HashMap<String, String> {
        &self.env_vars
    }

    pub(crate) fn builtin(&self) -> &HashMap<String, BuiltinFn> {
        &self.builtin
    }

    pub(crate) fn command_loader(&mut self) -> &CommandLoader {
        &self.command_loader
    }

    pub fn exit(&mut self, code: i32) {
        self.should_exit = true;
        self.exit_code = code;
    }

    pub fn run(&mut self) {
        while !self.should_exit {
            print!("$ ");
            io::stdout().flush().unwrap();

            let mut line = String::new();
            io::stdin().read_line(&mut line).unwrap();

            let line = line.trim_end(); // 把命令末尾换行符裁切掉

            // Step 1: Lex
            let lexer = Lexer::new();
            let line = match lexer.lex(&line) {
                Ok(lexed) => lexed,
                Err(e) => {
                    eprintln!("Error occurred while lexing: {e}");
                    return;
                }
            };

            // Step 2: Expand
            let expander = Expander::new();
            let line = match expander.expand(self, line) {
                Ok(expanded) => expanded,
                Err(e) => {
                    eprintln!("Error occurred while expanding: {e}");
                    return;
                }
            };

            let _status = self.eval_line(line);
        }

        process::exit(self.exit_code);
    }

    fn eval_line(&mut self, line: Vec<String>) -> i32 {
        if line.is_empty() {
            return 0;
        }

        let cmd = &line[0];
        let argv = &line[1..];

        //先匹配 builtin command
        if let Some(builtin) = self.builtin().get(cmd) {
            let code = match builtin(self, argv) {
                Ok(result) => result.code,
                Err(err) => {
                    eprintln!("{err}");
                    err.code
                }
            };
            self.status = code;
            return code
        }

        // 再寻找 executable
        if let Some(dir) = self.command_loader().find_executable(&cmd) {
            match Command::new(&dir).arg0(&cmd).args(&argv[..]).status() {
                Ok(exit_status) => return exit_status.code().unwrap(),
                Err(err) => {
                    eprintln!("Error occurred while invoking external command: {err}");
                    return 1;
                }
            }
        } 
            
        println!("{cmd}: command not found");
        return 1;
    }
}
