mod builtin;
mod external;
mod shell;
use std::{collections::HashMap, env, path::Path};

use crate::shell::Shell;

fn main() {
    let mut env_vars = HashMap::new();
    for (k, v) in env::vars() {
        env_vars.insert(k, v);
    }

    let mut shell = Shell::new(
        Path::new("./"),
        env_vars,
        [
            ("exit", builtin::exit),
            ("echo", builtin::echo),
            ("type", builtin::type_command),
        ],
    );

    shell.run();
}
