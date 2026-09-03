use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    builtin::BuiltinFn,
    external::CommandLoader,
    job_control::{JobControl, JobControlError},
};

#[derive(Debug)]
pub(crate) enum ResolvedCommand {
    Builtin(BuiltinFn),
    External(PathBuf),
}

pub struct Shell {
    // Current working directory
    current_dir: PathBuf,
    // Environment variables
    env: HashMap<String, String>,
    // Builtin commands
    builtin: HashMap<String, BuiltinFn>,
    command_loader: CommandLoader,
    job_control: JobControl,
    exit_request: Option<i32>,
    last_status: i32,
}

impl Shell {
    /// 使用当前目录、环境变量和内建命令表创建 Shell 会话。
    ///
    /// # Arguments
    ///
    /// * `current_dir` - Shell 初始当前目录。
    /// * `env` - Shell 持有的环境变量。
    /// * `builtin` - 可按名称解析的内建命令表。
    ///
    /// # Returns
    ///
    /// 尚未请求退出且最后状态码为 0 的新 Shell 会话。
    pub fn new<const N: usize>(
        current_dir: impl Into<PathBuf>,
        env: HashMap<String, String>,
        builtin: [(impl Into<String>, BuiltinFn); N],
    ) -> Self {
        let command_loader = CommandLoader::new(&env);
        Self {
            current_dir: current_dir.into(),
            env,
            builtin: builtin.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            command_loader,
            job_control: JobControl::new(),
            exit_request: None,
            last_status: 0,
        }
    }

    /// 返回 Shell 当前目录。
    ///
    /// # Returns
    ///
    /// 当前目录路径的借用。
    pub(crate) fn current_dir(&self) -> &Path {
        self.current_dir.as_path()
    }

    /// 更新 Shell 当前目录。
    ///
    /// # Arguments
    ///
    /// * `dir` - 新的当前目录。
    pub(crate) fn set_current_dir(&mut self, dir: impl Into<PathBuf>) {
        self.current_dir = dir.into();
    }

    /// 更新最近一次求值的状态码。
    ///
    /// # Arguments
    ///
    /// * `status` - 需要保存的 Shell 状态码。
    pub(crate) fn set_last_status(&mut self, status: i32) {
        self.last_status = status;
    }

    /// 返回 Shell 环境变量表。
    ///
    /// # Returns
    ///
    /// 环境变量名称和值的只读映射。
    pub(crate) fn environment(&self) -> &HashMap<String, String> {
        &self.env
    }

    /// 按名称查询单个环境变量。
    ///
    /// # Arguments
    ///
    /// * `name` - 环境变量名称。
    ///
    /// # Returns
    ///
    /// 环境变量存在时返回其值，否则返回 [`None`]。
    pub(crate) fn env(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(String::as_str)
    }

    /// 按名称解析内建命令或外部可执行文件。
    ///
    /// # Arguments
    ///
    /// * `name` - 待解析的命令名。
    ///
    /// # Returns
    ///
    /// 优先返回同名内建命令，其次返回 `PATH` 中的外部命令；均不存在时返回 [`None`]。
    pub(crate) fn resolve_command(&self, name: &str) -> Option<ResolvedCommand> {
        // 内建命令
        if let Some(builtin_command) = self.builtin.get(name).copied() {
            return Some(ResolvedCommand::Builtin(builtin_command));
        }

        // 外部可执行程序
        if let Some(external_command) = self.command_loader.find_executable(name, self.current_dir()) {
            return Some(ResolvedCommand::External(external_command));
        }

        // 都不可匹配返回None
        None
    }

    /// 为顶层 REPL 初始化交互式作业控制。
    ///
    /// 当标准输入不是终端时保留非交互模式，不执行终端所有权操作。
    ///
    /// # Errors
    ///
    /// 检查终端、配置 Shell 进程组/信号或取得控制终端失败时返回错误。
    pub(crate) fn initialize_job_control(&mut self) -> Result<(), JobControlError> {
        self.job_control.initialize_interactive()
    }

    /// 返回作业控制器的只读借用。
    ///
    /// # Returns
    ///
    /// 当前 Shell 唯一的 [`JobControl`] 实例。
    pub(crate) fn job_control(&self) -> &JobControl {
        &self.job_control
    }

    /// 返回作业控制器的可变借用。
    ///
    /// # Returns
    ///
    /// 当前 Shell 唯一 Job Control 状态的可变借用。
    pub(crate) fn job_control_mut(&mut self) -> &mut JobControl {
        &mut self.job_control
    }

    /// 创建供后台复合列表使用的隔离 Shell 上下文。
    ///
    /// 目录、环境和 builtin 表来自 fork 时的快照，修改不会回写父 Shell。
    ///
    /// # Arguments
    ///
    /// * `pgid` - 隔离 Shell 及其启动的所有进程所属的作业进程组。
    ///
    /// # Returns
    ///
    /// 不含父 Shell JobTable 和退出请求的独立 Shell 状态。
    pub(crate) fn forked_subshell(&self, pgid: nix::unistd::Pid) -> Self {
        let env = self.env.clone();
        let mut job_control = JobControl::new();
        job_control.become_subshell(pgid);
        Self {
            current_dir: self.current_dir.clone(),
            command_loader: CommandLoader::new(&env),
            env,
            builtin: self.builtin.clone(),
            job_control,
            exit_request: None,
            last_status: self.last_status,
        }
    }

    /// 结束 Shell 管理的后台作业并恢复终端状态。
    ///
    /// 清理采用 best-effort 语义，退出路径不会因信号或终端恢复失败而覆盖原始结果。
    pub(crate) fn shutdown_job_control(&mut self) {
        self.job_control.shutdown();
    }

    /// 记录退出请求及其状态码。
    ///
    /// # Arguments
    ///
    /// * `code` - Shell 退出状态码。
    pub fn request_exit(&mut self, code: i32) {
        self.exit_request = Some(code);
    }

    /// 判断当前会话是否已请求退出。
    ///
    /// # Returns
    ///
    /// 已记录退出请求时返回 `true`，否则返回 `false`。
    pub fn exit_requested(&self) -> bool {
        self.exit_request.is_some()
    }
}
