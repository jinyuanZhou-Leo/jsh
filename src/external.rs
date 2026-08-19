use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};

use is_executable::IsExecutable;

pub(crate) struct CommandLoader {
    dirs: Vec<PathBuf>,
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
    /// 持有按平台规则拆分后的命令搜索目录的加载器。
    pub fn new(env_vars: &HashMap<String, String>) -> Self {
        let dirs: Vec<PathBuf> = env_vars
            .get("PATH")
            // 用split_paths来支持跨平台
            .map(|val| env::split_paths(val).collect())
            .unwrap_or_default();
        Self {
            dirs,
            loaded_command: HashMap::new(),
        }
    }

    /// 在配置的搜索目录中查找同名可执行文件。
    ///
    /// # Arguments
    ///
    /// * `cmd` - 待查找的命令名。
    /// * `current_dir` - 解析相对命令路径和相对 `PATH` 目录时使用的当前目录。
    ///
    /// # Returns
    ///
    /// 找到时返回第一个可执行文件的路径，否则返回 [`None`]。
    pub fn find_executable(&self, cmd: &str, current_dir: &Path) -> Option<PathBuf> {
        if cmd.contains('/') {
            // https://github.com/jinyuanZhou-Leo/jsh/issues/4
            // 正确处理包含相对/绝对路径的命令
            let candidate = current_dir.join(cmd);
            return (candidate.is_file() && candidate.is_executable()).then_some(candidate);
        }

        self.dirs
            .iter()
            // https://github.com/jinyuanZhou-Leo/jsh/issues/4
            // 1. 相对 PATH 目录接到 current_dir；绝对 PATH 目录保持不变
            // (`Path::join` 会拼接相对目录，并用绝对目录替换左侧)
            // 2. 再拼上命令名，得到候选可执行文件路径
            .map(|dir| current_dir.join(dir).join(cmd))
            .find_map(|candidate| {
                (candidate.is_file() && candidate.is_executable()).then_some(candidate)
            })
    }
}
