use crate::{builtin::{BuiltinIo, BuiltinOutput}, shell::Shell};

pub fn exit(shell: &mut Shell, _argv: &[String],_io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    shell.request_exit(0);
    Ok(0)
}
