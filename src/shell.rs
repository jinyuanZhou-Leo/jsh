use std::{
    collections::HashMap, io::{self, Write}, path::{Path, PathBuf}, process::{self, Command}, sync::mpsc, thread::{self, spawn},
};

use crate::external::CommandLoader;

/// Shell: shell context
/// &[String]: command arguments
pub type BuiltinFn = fn(&mut Shell, &[String]) -> BuiltinResult;

pub struct BuiltinResult {
    pub code: i32,
}

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

            let mut command_line = String::new();
            io::stdin().read_line(&mut command_line).unwrap();

            let _status = self.eval_line(command_line.trim_end());
        }

        process::exit(self.exit_code);
    }

    fn eval_line(&mut self, line: &str) -> i32 {
        let args: Vec<String> = line.split_whitespace().map(|x| x.to_string()).collect();

        if args.is_empty() {
            return 0;
        }

        let cmd = &args[0];
        let argv = &args[0..];

        if let Some(builtin) = self.builtin().get(cmd) {
            let code = builtin(self, argv).code;
            self.status = code;
            code
        } else {
            if let Some(dir) = self.command_loader().find_executable(&cmd) {
                match Command::new(dir).args(argv[1..].iter()).status(){
                    Ok(exit_status) =>{
                        exit_status.code().unwrap()
                    },
                    Err(err) =>{
                        eprintln!("Error occurred while invoking external command: {err}");
                        1
                    }
                }
            }
            else{
                println!("{cmd}: command not found");
                1
            }
        }
    }
}
