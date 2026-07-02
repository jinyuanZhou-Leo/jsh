use crate::shell::{BuiltinResult, Shell};

pub fn pwd(shell: &mut Shell, _argv: &[String]) -> BuiltinResult {
    println!("{}", shell.pwd().display());
    BuiltinResult { code: 0 }
}
