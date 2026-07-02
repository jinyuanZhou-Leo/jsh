use crate::shell::{BuiltinResult, Shell};

pub fn echo(_shell: &mut Shell, argv: &[String]) -> BuiltinResult {
    println!("{}", argv.join(" "));
    BuiltinResult { code: 0 }
}
