use crate::shell::{BuiltinResult, Shell};

pub fn echo(_shell: &mut Shell, argv: &[String]) -> BuiltinResult {
    let argv = &argv[1..];
    println!("{}", argv.join(" "));
    BuiltinResult { code: 0 }
}
