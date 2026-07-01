use crate::shell::{BuiltinResult, Shell};

pub fn exit(shell: &mut Shell, _argv: &[String]) -> BuiltinResult {
    shell.exit(0);
    BuiltinResult { code: 0 }
}
