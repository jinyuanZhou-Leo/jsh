use crate::{
    builtin::{BuiltinError, BuiltinIo, BuiltinOutput},
    shell::Shell,
};

/// 将 Shell 当前目录切换到唯一的目录参数。
///
/// # Arguments
///
/// * `shell` - 保存并更新当前目录的 Shell 上下文。
/// * `argv` - 必须恰好包含一个目标目录的参数列表。
/// * `_io` - 当前实现不使用的内建命令 I/O。
///
/// # Returns
///
/// 切换成功时返回状态码 0。
///
/// # Errors
///
/// 参数数量不正确、目标路径无法规范化或目标不是目录时返回 [`BuiltinError`]。
pub fn cd(shell: &mut Shell, argv: &[String], _io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    match argv {
        [path] => {
            let path = shell.current_dir().join(path);

            // canonicalize 同时也检查了路径是否存在
            let path = match path.canonicalize() {
                Ok(path) => path,
                Err(_) => {
                    return Err(BuiltinError::new(
                        1,
                        format!("cd: {0}: No such file or directory", path.display()),
                    ));
                }
            };

            if !path.is_dir() {
                return Err(BuiltinError::new(
                    1,
                    format!("cd: {0}: Not a directory", path.display()),
                ));
            }

            shell.set_current_dir(path);
            Ok(0)
        }
        [] => Err(BuiltinError::new(1, "cd: missing operand")),
        _ => Err(BuiltinError::new(1, "cd: too many arguments")),
    }
}
