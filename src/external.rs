use std::{collections::HashMap, env, path::PathBuf};

use is_executable::IsExecutable;

#[derive(Default)]
pub(crate) struct CommandLoader {
    path: Vec<PathBuf>,
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
        let path: Vec<PathBuf> = env_vars
            .get("PATH")
            // 用split_path来支持跨平台
            .map(|val| env::split_paths(val).collect())
            .unwrap_or_default();
        Self {
            path,
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
    pub fn find_executable(&self, cmd: &str) -> Option<PathBuf> {
        // 遍历PATH中的目录，找executable
        for dir in &self.path {
            let candidate = dir.join(cmd);

            if candidate.is_file() && candidate.is_executable() {
                return Some(candidate);
            }
        }

        None
    }
}
