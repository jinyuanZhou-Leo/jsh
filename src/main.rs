mod builtin;
mod external;
mod shell;
use std::{collections::HashMap, env};

use crate::shell::Shell;

fn main() {
    let mut env_vars = HashMap::new();
    for (k, v) in env::vars() {
        env_vars.insert(k, v);
    }

    let mut shell = Shell::new(
        env::current_dir().unwrap(),
        env_vars,
        [
            ("exit", builtin::exit),
            ("echo", builtin::echo),
            ("type", builtin::type_command),
            ("pwd", builtin::pwd),
            ("cd", builtin::cd),
        ],
    );

    shell.run();
}
