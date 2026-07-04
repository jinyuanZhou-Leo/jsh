use crate::shell::{BuiltinOutput, BuiltinResult, Shell};

pub fn exit(shell: &mut Shell, _argv: &[String]) -> BuiltinOutput {
    shell.exit(0);
    Ok(BuiltinResult { code: 0 })
}
