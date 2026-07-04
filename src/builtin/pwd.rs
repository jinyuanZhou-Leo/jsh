use crate::shell::{BuiltinOutput, BuiltinResult, Shell};

pub fn pwd(shell: &mut Shell, _argv: &[String]) -> BuiltinOutput {
    println!("{}", shell.pwd().display());
    Ok(BuiltinResult { code: 0 })
}
