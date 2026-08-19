use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{builtin::BuiltinFn, external::CommandLoader};

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
    /// * `path` - 新的当前目录路径。
    pub(crate) fn set_current_dir(&mut self, path: impl Into<PathBuf>) {
        self.current_dir = path.into();
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
