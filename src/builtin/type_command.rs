use crate::shell::{BuiltinResult, Shell};

pub fn type_command(shell: &mut Shell, argv: &[String]) -> BuiltinResult {
    match argv {
        [command] => {
            if let Some(_builtin) = shell.builtin().get(command) {
                println!("{} is a shell builtin", command);
                BuiltinResult { code: 0 }
            } else {
                if let Some(dir) = shell.command_loader().find_executable(command) {
                    println!("{} is {}", command, dir.to_str().unwrap());
                    BuiltinResult { code: 0 }
                } else {
                    println!("{}: not found", command);
                    BuiltinResult { code: 0 }
                }
            }
        }
        [] => {
            println!("type: missing operand");
            BuiltinResult { code: 1 }
        }
        _ => {
            println!("type: too many arguments");
            BuiltinResult { code: 1 }
        }
    }
}
