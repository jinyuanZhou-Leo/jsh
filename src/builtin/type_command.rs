use crate::{builtin::{BuiltinError, BuiltinIo, BuiltinOutput}, shell::{ResolvedCommand, Shell}};

pub fn type_command(shell: &mut Shell, argv: &[String], io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    match argv {
        [command_name] => {
            match shell.resolve_command(command_name) {
                Some(ResolvedCommand::Builtin(_)) => {
                    writeln!(io.stdout(), "{} is a shell builtin", command_name)?;
                },
                Some(ResolvedCommand::External(path)) => {
                    writeln!(io.stdout(), "{} is {}", command_name, path.display())?;
                },
                None => {
                    writeln!(io.stderr(), "{}: not found", command_name)?;
                }
            }

            Ok(0)
        }
        [] => Err(BuiltinError::new(1, "type: missing operand")),
        _ => Err(BuiltinError::new(1, "type: too many arguments")),
    }
}
