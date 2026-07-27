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
            command_loader: command_loader,
            exit_request: None,
            last_status: 0,
        }
    }

    /// 返回当前目录
    pub(crate) fn current_dir(&self) -> &Path {
        self.current_dir.as_path()
    }

    // 设置当前目录
    pub(crate) fn set_current_dir(&mut self, path: impl Into<PathBuf>) {
        self.current_dir = path.into();
    }

    pub(crate) fn set_last_status(&mut self, status: i32) {
        self.last_status = status;
    }

    // 获取环境变量哈希表
    pub(crate) fn environment(&self) -> &HashMap<String, String> {
        &self.env
    }

    /// 环境变量单值查询
    pub(crate) fn env(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(String::as_str)
    }

    /// 根据名称解析命令
    pub(crate) fn resolve_command(&self, name: &str) -> Option<ResolvedCommand> {
        // 内建命令
        if let Some(builtin_command) = self.builtin.get(name).copied() {
            return Some(ResolvedCommand::Builtin(builtin_command));
        }

        // 外部可执行程序
        if let Some(external_command) = self.command_loader.find_executable(name) {
            return Some(ResolvedCommand::External(external_command));
        }

        // 都不可匹配返回None
        None
    }

    /// 要求退出
    pub fn request_exit(&mut self, code: i32) {
        self.exit_request = Some(code);
    }

    pub fn exit_requested(&self) -> bool {
        match self.exit_request {
            Some(_) => true,
            None => false,
        }
    }
}
