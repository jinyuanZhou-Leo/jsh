use crate::shell::{BuiltinOutput, BuiltinResult, Shell};

pub fn echo(_shell: &mut Shell, argv: &[String]) -> BuiltinOutput {
    println!("{}", argv.join(" "));
    Ok(BuiltinResult { code: 0 })
}
