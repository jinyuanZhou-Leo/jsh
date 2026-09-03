use crate::{
    builtin::{BuiltinError, BuiltinIo, BuiltinOutput},
    shell::Shell,
};

/// 将一个作业移到前台并等待其停止或结束。
///
/// # Arguments
///
/// * `shell` - 持有目标 JobTable 和终端会话的当前 Shell。
/// * `argv` - 可选的 `%<job-id>`；省略时选择 current job。
/// * `_io` - builtin I/O 上下文；目标作业继续使用其启动时配置的标准流。
///
/// # Returns
///
/// 作业停止或结束后返回其最后阶段的 Shell 状态码。
///
/// # Errors
///
/// 参数过多、jobspec 无效、目标不存在，或恢复/等待作业失败时返回 [`BuiltinError`]。
pub fn fg(shell: &mut Shell, argv: &[String], _io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    let specification = match argv {
        [] => None,
        [specification] => Some(specification.as_str()),
        _ => return Err(BuiltinError::new(1, "fg: too many arguments")),
    };

    shell
        .job_control_mut()
        .continue_foreground(specification)
        .map_err(|error| BuiltinError::new(1, format!("fg: {error}")))
}
