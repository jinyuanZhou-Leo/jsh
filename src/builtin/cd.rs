use std::path::PathBuf;

use crate::{
    builtin::{BuiltinError, BuiltinIo, BuiltinOutput},
    shell::Shell,
};

pub fn cd(shell: &mut Shell, argv: &[String], _io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    match argv {
        [dir] => {
            let dir = PathBuf::from(dir);
            let dir = if dir.is_absolute() {
                dir
            } else {
                shell.current_dir().join(dir)
            };
            
            // canonical同时也检查了路径是否存在
            let dir = match dir.canonicalize() {
                Ok(dir) => dir,
                Err(_) => return Err(BuiltinError::new(1, "No such file or directory"))
            };

            // 检查路径是否为目录
            if !dir.is_dir() {
                return Err(BuiltinError::new(1, "Not a directory"));
            }

            shell.set_current_dir(dir);
            Ok(0)
        }
        [] => Err(BuiltinError::new(1, "cd: missing operand")),
        _ => Err(BuiltinError::new(1, "cd: too many arguments")),
    }
}
