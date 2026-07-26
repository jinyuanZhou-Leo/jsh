use crate::{builtin::{BuiltinIo, BuiltinOutput}, shell::Shell};

pub fn echo(_shell: &mut Shell, argv: &[String], io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    writeln!(io.stdout(), "{}", argv.join(" "))?;
    Ok(0)
}
