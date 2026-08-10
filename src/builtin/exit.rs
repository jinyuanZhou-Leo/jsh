use crate::{
    builtin::{BuiltinIo, BuiltinOutput},
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
pub fn exit(shell: &mut Shell, _argv: &[String], _io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    shell.request_exit(0);
    Ok(0)
}
