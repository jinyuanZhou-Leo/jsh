use std::{error::Error, path::PathBuf};

use crate::shell::{BuiltinError, BuiltinOutput, BuiltinResult, Shell};

pub fn cd(shell: &mut Shell, argv: &[String]) -> BuiltinOutput {
    match argv {
        [dir] => match resolve_cd_path(shell, dir) {
            Ok(new_path) => {
                shell.set_pwd(new_path);
                Ok(BuiltinResult { code: 0 })
            }
            Err(e) => Err(BuiltinError::new(1, format!("cd: {}: {}", dir, e))),
        },
        [] => Err(BuiltinError::new(1, "cd: missing operand")),
        _ => Err(BuiltinError::new(1, "cd: too many arguments")),
    }
}

fn expand_tilde(shell: &Shell, path: &str) -> Result<PathBuf, Box<dyn Error>> {
    let Some(home_dir) = shell.env_vars().get("HOME") else {
        return Err("HOME not found".into());
    };

    let expanded = path.replacen("~", home_dir, 1);
    Ok(PathBuf::from(expanded))
}

fn resolve_cd_path(shell: &Shell, dir: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = if dir.starts_with('~') {
        expand_tilde(shell, dir)?
    } else {
        PathBuf::from(dir)
    };

    let dir = if dir.is_absolute() {
        dir
    } else {
        shell.pwd().join(dir)
    };

    let dir = match dir.canonicalize() {
        Ok(dir) => dir,
        Err(_) => return Err("No such file or directory".into()),
    }; // canonical同时也检查了是否存在

    if !dir.is_dir() {
        return Err("Not a directory".into());
    }

    Ok(dir)
}
