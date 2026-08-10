use crate::{
    builtin::{BuiltinIo, BuiltinOutput},
    shell::Shell,
};

/// 将 Shell 当前目录写入标准输出。
///
/// # Arguments
///
/// * `shell` - 提供当前目录的 Shell 上下文。
/// * `_argv` - 当前实现忽略的参数列表。
/// * `io` - 提供标准输出的内建命令 I/O。
///
/// # Returns
///
/// 写入成功时返回状态码 0。
///
/// # Errors
///
/// 标准输出写入失败时返回 [`crate::builtin::BuiltinError::Io`]。
pub fn pwd(shell: &mut Shell, _argv: &[String], io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    writeln!(io.stdout(), "{}", shell.current_dir().display())?;
    Ok(0)
}
