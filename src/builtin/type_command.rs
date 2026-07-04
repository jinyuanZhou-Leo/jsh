use crate::shell::{BuiltinError, BuiltinOutput, BuiltinResult, Shell};

pub fn type_command(shell: &mut Shell, argv: &[String]) -> BuiltinOutput {
    match argv {
        [command] => {
            if let Some(_builtin) = shell.builtin().get(command) {
                println!("{} is a shell builtin", command);
                Ok(BuiltinResult { code: 0 })
            } else {
                if let Some(dir) = shell.command_loader().find_executable(command) {
                    println!("{} is {}", command, dir.to_str().unwrap());
                    Ok(BuiltinResult { code: 0 })
                } else {
                    println!("{}: not found", command);
                    Ok(BuiltinResult { code: 0 })
                }
            }
        }
        [] => Err(BuiltinError::new(1, "type: missing operand")),
        _ => Err(BuiltinError::new(1, "type: too many arguments")),
    }
}
