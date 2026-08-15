use crate::{
    builtin::{BuiltinError, BuiltinIo, BuiltinOutput},
    shell::Shell,
};

/// 请求当前 Shell 会话以状态码 0 退出。
///
/// # Arguments
///
/// * `shell` - 记录退出请求的 Shell 上下文。
/// * `_argv` - 当前实现忽略的参数列表。
/// * `_io` - 当前实现不使用的内建命令 I/O。
///
/// # Returns
///
/// 始终返回状态码 0。
pub fn exit(shell: &mut Shell, argv: &[String], _io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    match argv {
        [exit_code] => {
            if exit_code.chars().all(|c| {c.is_ascii_digit()}){
                let exit_code = exit_code.parse::<i32>().map_err(|_| BuiltinError::new(1, "exit: exit code out of bound"))?;
                shell.request_exit(exit_code);
                Ok(0)
            } else {
                Err(BuiltinError::new(1, format!("exit: unknown operand `{exit_code}`")))
            }
            
        }
        [] => {
            shell.request_exit(0);
            Ok(0)
        }
        _ => Err(BuiltinError::new(1, "exit: too many arguments")),
    }
}
