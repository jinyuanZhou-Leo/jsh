use std::{collections::HashMap, env, path::PathBuf};

use is_executable::IsExecutable;

#[derive(Default)]
pub(crate) struct CommandLoader {
    path: Vec<PathBuf>,
    loaded_command: HashMap<String, PathBuf>,
}

impl CommandLoader {
    pub fn new(env_vars: &HashMap<String, String>) -> Self {
        let path: Vec<PathBuf> = env_vars
            .get("PATH")
            // 用split_path来支持跨平台
            .map(|val| env::split_paths(val).collect())
            .unwrap_or_default();
        Self {
            path: path,
            loaded_command: HashMap::new(),
        }
    }

    pub fn find_executable(&self, cmd: &str) -> Option<PathBuf> {
        for dir in &self.path {
            let candidate = dir.join(cmd);

            if candidate.is_file() && candidate.is_executable() {
                return Some(candidate);
            }
        }

        None
    }
}
