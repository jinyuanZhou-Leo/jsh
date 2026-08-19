use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};

use is_executable::IsExecutable;

pub(crate) struct CommandLoader {
    paths: Vec<PathBuf>,
    loaded_command: HashMap<String, PathBuf>,
}

impl CommandLoader {
    /// 从环境变量中的 `PATH` 创建命令加载器。
    ///
    /// # Arguments
    ///
    /// * `env_vars` - 用于读取 `PATH` 的 Shell 环境变量表。
    ///
    /// # Returns
    ///
    /// 持有按平台规则拆分后的命令搜索路径的加载器。
    pub fn new(env_vars: &HashMap<String, String>) -> Self {
        let paths: Vec<PathBuf> = env_vars
            .get("PATH")
            // 用split_path来支持跨平台
            .map(|val| env::split_paths(val).collect())
            .unwrap_or_default();
        Self {
            paths,
            loaded_command: HashMap::new(),
        }
    }

    /// 在配置的搜索路径中查找同名可执行文件。
    ///
    /// # Arguments
    ///
    /// * `cmd` - 待查找的命令名。
    ///
    /// # Returns
    ///
    /// 找到时返回第一个可执行文件的路径，否则返回 [`None`]。
    pub fn find_executable(&self, cmd: &str, cwd: &Path) -> Option<PathBuf> {
        if cmd.contains('/') {
            // https://github.com/jinyuanZhou-Leo/jsh/issues/4
            // 正确处理包含相对/绝对路径的命令
            let candidate = cwd.join(cmd);
            return (candidate.is_file() && candidate.is_executable()).then_some(candidate);
        }

        self.paths
            .iter()
            // https://github.com/jinyuanZhou-Leo/jsh/issues/4
            // 1. 正确处理包含相对/绝对路径的PATH变量
            // (标准库中的join函数可以同时处理path参数是绝对或者相对两种情况)
            // 2. 尾部拼入command_name
            .map(|candidate| cwd.join(candidate).join(cmd))
            .find_map(|candidate| {
                (candidate.is_file() && candidate.is_executable()).then_some(candidate)
            })
    }
}
