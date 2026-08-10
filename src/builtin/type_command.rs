use crate::{
    builtin::{BuiltinError, BuiltinIo, BuiltinOutput},
    shell::{ResolvedCommand, Shell},
};

/// 判断一个命令名对应内建命令、外部命令还是未知命令。
///
/// # Arguments
///
/// * `shell` - 提供命令解析能力的 Shell 上下文。
/// * `argv` - 必须恰好包含一个待查询命令名的参数列表。
/// * `io` - 提供标准输出和标准错误的内建命令 I/O。
///
/// # Returns
///
/// 查询完成时返回状态码 0。
///
/// # Errors
///
/// 参数数量不正确或输出写入失败时返回 [`BuiltinError`]。
pub fn type_command(shell: &mut Shell, argv: &[String], io: &mut BuiltinIo<'_>) -> BuiltinOutput {
    match argv {
        [command_name] => match shell.resolve_command(command_name) {
            Some(ResolvedCommand::Builtin(_)) => {
                writeln!(io.stdout(), "{} is a shell builtin", command_name)?;
                Ok(0)
            }
            Some(ResolvedCommand::External(path)) => {
                writeln!(io.stdout(), "{} is {}", command_name, path.display())?;
                Ok(0)
            }
            None => {
                writeln!(io.stderr(), "{}: not found", command_name)?;
                Ok(1)
            }
        },
        [] => Err(BuiltinError::new(1, "type: missing operand")),
        _ => Err(BuiltinError::new(1, "type: too many arguments")),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, io};

    use super::type_command;
    use crate::{
        builtin::{BuiltinFn, BuiltinIo},
        shell::Shell,
    };

    #[test]
    fn missing_command_returns_failure_status() {
        let mut shell = Shell::new(".", HashMap::new(), [] as [(&str, BuiltinFn); 0]);
        let args = vec!["missing-command".to_owned()];
        let mut stdin = io::empty();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut builtin_io = BuiltinIo::new(&mut stdin, &mut stdout, &mut stderr);

        let status = type_command(&mut shell, &args, &mut builtin_io)
            .expect("type should report the missing command");

        assert_eq!(status, 1);
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "missing-command: not found\n"
        );
    }
}
