use crate::{
    builtin::{BuiltinError, BuiltinIo, BuiltinOutput},
    shell::Shell,
};

/// 输出当前 Shell 管理的作业。
///
/// # Arguments
///
/// * `shell` - 持有待刷新 JobTable 的当前 Shell。
/// * `argv` - `jobs` 的参数；当前实现不接受任何参数。
/// * `io` - 接收作业列表或 builtin 错误的标准流上下文。
///
/// # Returns
///
/// 成功刷新并输出全部作业时返回状态码 0。
///
/// # Errors
///
/// 收到多余参数、回收子进程状态失败或写入输出失败时返回 [`BuiltinError`]。
pub fn jobs(shell: &mut Shell, argv: &[String], io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    if !argv.is_empty() {
        return Err(BuiltinError::new(1, "jobs: too many arguments"));
    }

    let lines = shell
        .job_control_mut()
        .list_jobs()
        .map_err(|error| BuiltinError::new(1, format!("jobs: {error}")))?;
    for line in lines {
        writeln!(io.stdout(), "{line}")?;
    }
    Ok(0)
}
