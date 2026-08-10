use crate::{
    builtin::{BuiltinIo, BuiltinOutput},
    shell::Shell,
};

/// 将参数以单个空格连接，并向标准输出写入一行。
///
/// # Arguments
///
/// * `_shell` - 当前实现不使用的 Shell 上下文。
/// * `argv` - 需要输出的参数列表。
/// * `io` - 提供标准输出的内建命令 I/O。
///
/// # Returns
///
/// 写入成功时返回状态码 0。
///
/// # Errors
///
/// 标准输出写入失败时返回 [`crate::builtin::BuiltinError::Io`]。
pub fn echo(_shell: &mut Shell, argv: &[String], io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    writeln!(io.stdout(), "{}", argv.join(" "))?;
    Ok(0)
}
