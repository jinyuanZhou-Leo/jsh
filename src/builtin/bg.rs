use crate::{
    builtin::{BuiltinError, BuiltinIo, BuiltinOutput},
    shell::Shell,
};

/// 在后台继续一个已停止的作业。
///
/// # Arguments
///
/// * `shell` - 持有目标 JobTable 的当前 Shell。
/// * `argv` - 可选的 `%<job-id>`；省略时选择 current stopped job。
/// * `io` - 用于输出恢复后作业状态的 builtin I/O 上下文。
///
/// # Returns
///
/// 成功发送 `SIGCONT` 并输出作业状态时返回状态码 0。
///
/// # Errors
///
/// 参数过多、jobspec 无效、目标不存在/未停止，或发送信号、写入输出失败时返回 [`BuiltinError`]。
pub fn bg(shell: &mut Shell, argv: &[String], io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    let specification = match argv {
        [] => None,
        [specification] => Some(specification.as_str()),
        _ => return Err(BuiltinError::new(1, "bg: too many arguments")),
    };

    let line = shell
        .job_control_mut()
        .continue_background(specification)
        .map_err(|error| BuiltinError::new(1, format!("bg: {error}")))?;
    writeln!(io.stdout(), "{line}")?;
    Ok(0)
}
