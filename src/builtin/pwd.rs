use crate::{builtin::{BuiltinIo, BuiltinOutput}, shell::Shell};

pub fn pwd(shell: &mut Shell, _argv: &[String], io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    writeln!(io.stdout(), "{}", shell.current_dir().display())?;
    Ok(0)
}
