use std::{error::Error, path::PathBuf};

use crate::shell::{BuiltinResult, Shell};

pub fn cd(shell: &mut Shell, argv: &[String]) -> BuiltinResult {
    match argv {
        [dir] => match resolve_cd_path(shell, dir) {
            Ok(new_path) => {
                shell.set_pwd(new_path);
                BuiltinResult { code: 0 }
            }
            Err(e) => {
                println!("cd: {}: {}", dir, e);
                BuiltinResult { code: 1 }
            }
        },
        [] => {
            println!("cd: missing operand");
            BuiltinResult { code: 1 }
        }
        _ => {
            println!("cd: too many arguments");
            BuiltinResult { code: 1 }
        }
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
        Ok(dir)=>dir,
        Err(_) => return Err("No such file or directory".into())
    }; // canonical同时也检查了是否存在

    if !dir.is_dir() {
        return Err("Not a directory".into());
    }

    Ok(dir)
}
