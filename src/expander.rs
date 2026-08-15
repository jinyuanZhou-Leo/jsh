use std::{collections::HashMap, env::home_dir};
use thiserror::Error;

use crate::{
    parser::Command,
    token::{RedirectOperator, Word, WordPart},
};

pub(crate) struct ExpandedRedirection {
    pub(crate) redirected_fd: u32,
    pub(crate) operator: RedirectOperator,
    pub(crate) operand: ExpandedRedirectOperand,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExpandedRedirectOperand {
    Fd(u32),
    Path(String),
}

pub(crate) struct ExpandedCommand {
    pub(crate) args: Vec<String>,
    pub(crate) redirections: Vec<ExpandedRedirection>,
}

pub(crate) struct Expander<'env> {
    env: &'env HashMap<String, String>,
}

// Notes: 不再提供直接Expand整个Ast的接口，而是在执行器中执行一条展开一条

impl<'env> Expander<'env> {
    /// 创建借用 Shell 环境变量的展开器。
    ///
    /// # Arguments
    ///
    /// * `env` - 展开过程可读取的环境变量表。
    ///
    /// # Returns
    ///
    /// 绑定到给定环境变量生命周期的 [`Expander`]。
    pub(crate) fn new(env: &'env HashMap<String, String>) -> Self {
        Self { env }
    }

    /// 展开命令参数和重定向操作数。
    ///
    /// # Arguments
    ///
    /// * `command` - Parser 生成的未展开命令。
    ///
    /// # Returns
    ///
    /// 参数和重定向均已展开的 [`ExpandedCommand`]。
    ///
    /// # Errors
    ///
    /// 波浪号无法展开，或文件描述符操作数不是有效 `u32` 时返回 [`ExpanderError`]。
    pub(crate) fn expand_command(self, command: Command) -> Result<ExpandedCommand, ExpanderError> {
        let Command { args, redirections } = command;

        // 展开过程不会改变数组长度，使用with_capacity减少不必要的堆内存分配
        let mut expanded_args = Vec::with_capacity(args.len());
        let mut expanded_redirections = Vec::with_capacity(redirections.len());

        // Word 展开
        for word in args {
            expanded_args.push(self.expand_word(word)?);
        }

        // 重定向展开
        for redirection in redirections {
            let operator = redirection.operator;
            // 先做operand展开然后在尝试解析具体的重定向行为
            let expanded_operand = self.expand_word(redirection.operand)?;
            let operand = match operator {
                RedirectOperator::DuplicateInput | RedirectOperator::DuplicateOutput => {
                    // 解析右侧fd
                    let fd = expanded_operand.parse::<u32>().map_err(|_| {
                        ExpanderError::InvalidFileDescriptor(expanded_operand.clone())
                    })?;
                    ExpandedRedirectOperand::Fd(fd)
                }
                RedirectOperator::Input
                | RedirectOperator::OutputTruncate
                | RedirectOperator::OutputAppend => {
                    // 输入输出重定向，存入文件路径
                    ExpandedRedirectOperand::Path(expanded_operand)
                }
            };

            expanded_redirections.push(ExpandedRedirection {
                redirected_fd: redirection.redirected_fd,
                operator,
                operand,
            });
        }

        Ok(ExpandedCommand {
            args: expanded_args,
            redirections: expanded_redirections,
        })
    }

    /// 按顺序连接并展开一个 Word 中的所有组成部分。
    ///
    /// # Arguments
    ///
    /// * `word` - 保留引用和转义边界的未展开 Word。
    ///
    /// # Returns
    ///
    /// 展开后的字符串。
    ///
    /// # Errors
    ///
    /// 词首波浪号无法解析到主目录时返回 [`ExpanderError::CouldNotExpandTilde`]。
    pub(crate) fn expand_word(&self, word: Word) -> Result<String, ExpanderError> {
        let mut buffer = String::new();
        for (idx, part) in word.into_parts().enumerate() {
            match part {
                WordPart::Unquoted(content) => {
                    if idx == 0 && (content == "~" || content.starts_with("~/")) {
                        // 仅对词首第一个 "~" 或以 "~/" 开头的 WordPart 做展开
                        buffer.push_str(&self.expand_tilde(&content)?);
                    } else {
                        buffer.push_str(&content);
                    }
                }
                WordPart::SingleQuoted(literal) => {
                    // 字面量
                    buffer.push_str(&literal);
                }
                WordPart::DoubleQuoted(content) => {
                    // TODO: treat as literal FOR NOW
                    buffer.push_str(&content);
                }
                WordPart::Escaped(escaped_char) => {
                    buffer.push(escaped_char);
                }
            }
        }

        Ok(buffer)
    }

    /// 使用当前用户主目录展开字符串中的波浪号。
    ///
    /// # Arguments
    ///
    /// * `input` - 包含待展开波浪号的输入字符串。
    ///
    /// # Returns
    ///
    /// 将波浪号替换为主目录路径后的字符串。
    ///
    /// # Errors
    ///
    /// 无法取得主目录，或主目录路径不是有效 UTF-8 时返回
    /// [`ExpanderError::CouldNotExpandTilde`]。
    fn expand_tilde(&self, input: &str) -> Result<String, ExpanderError> {
        // 使用env库获取home_dir以提供多平台的支持
        let home_dir = home_dir()
            .ok_or(ExpanderError::CouldNotExpandTilde)?;
        let home_dir = home_dir
            .to_str()
            .ok_or(ExpanderError::CouldNotExpandTilde)?;

        let suffix = input.strip_prefix('~').ok_or(ExpanderError::CouldNotExpandTilde)?;
        
        Ok(format!("{home_dir}{suffix}"))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum ExpanderError {
    // #[error("Could not found env var `{0}`")]
    // EnvironmentVariableNotFound(&'static str),
    #[error("Could not derive tilde, please check environment variables")]
    CouldNotExpandTilde,
    #[error("invalid file descriptor `{0}`")]
    InvalidFileDescriptor(String),
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, env::home_dir, sync::OnceLock};

    use super::{ExpandedRedirectOperand, Expander};
    use crate::{
        parser::{Command, Redirection},
        token::{RedirectOperator, Word, WordPart},
    };

    fn expander() -> Expander<'static> {
        // The current Expander keeps the environment for future variable expansion.
        static ENV: OnceLock<HashMap<String, String>> = OnceLock::new();
        Expander::new(ENV.get_or_init(HashMap::new))
    }

    #[test]
    fn expands_all_word_parts_in_order() {
        let word = Word::from_parts(vec![
            WordPart::Unquoted("plain".into()),
            WordPart::SingleQuoted(" single ".into()),
            WordPart::DoubleQuoted("double".into()),
            WordPart::Escaped('!'),
        ]);

        assert_eq!(
            expander().expand_word(word),
            Ok("plain single double!".into())
        );
    }

    #[test]
    fn expands_tilde_only_at_the_start_of_the_first_unquoted_part() {
        let home = home_dir().expect("test environment should have a home directory");
        let word = Word::from_parts(vec![
            WordPart::Unquoted("~/project".into()),
            WordPart::Unquoted("/~literal".into()),
        ]);

        assert_eq!(
            expander().expand_word(word),
            Ok(format!("{}/project/~literal", home.display()))
        );
    }

    #[test]
    fn does_not_expand_a_quoted_or_non_initial_tilde() {
        let quoted = Word::from_parts(vec![WordPart::SingleQuoted("~/project".into())]);
        let non_initial = Word::from_parts(vec![
            WordPart::Unquoted("prefix".into()),
            WordPart::Unquoted("~/project".into()),
        ]);
        let named_user = Word::from_parts(vec![WordPart::Unquoted("~x/project".into())]);

        assert_eq!(expander().expand_word(quoted), Ok("~/project".into()));
        assert_eq!(
            expander().expand_word(non_initial),
            Ok("prefix~/project".into())
        );
        assert_eq!(expander().expand_word(named_user), Ok("~x/project".into()));
    }

    #[test]
    fn expands_only_the_leading_tilde() {
        let home = home_dir().expect("test environment should have a home directory");
        let word = Word::from_parts(vec![WordPart::Unquoted("~/a~b".into())]);

        assert_eq!(
            expander().expand_word(word),
            Ok(format!("{}/a~b", home.display()))
        );
    }

    #[test]
    fn expands_command_arguments_and_redirection_operands() {
        let command = Command {
            args: vec![
                Word::from_parts(vec![WordPart::Unquoted("echo".into())]),
                Word::from_parts(vec![WordPart::SingleQuoted("hello world".into())]),
            ],
            redirections: vec![Redirection {
                redirected_fd: 2,
                operator: RedirectOperator::OutputAppend,
                operand: Word::from_parts(vec![WordPart::Unquoted("error.log".into())]),
            }],
        };

        let expanded = expander()
            .expand_command(command)
            .expect("command should expand");

        assert_eq!(expanded.args, vec!["echo", "hello world"]);
        assert_eq!(expanded.redirections.len(), 1);
        assert_eq!(expanded.redirections[0].redirected_fd, 2);
        assert_eq!(
            expanded.redirections[0].operator,
            RedirectOperator::OutputAppend
        );
        assert_eq!(
            expanded.redirections[0].operand,
            ExpandedRedirectOperand::Path("error.log".into())
        );
    }

    #[test]
    fn resolves_redirection_operands_according_to_the_operator() {
        let command = Command {
            args: vec![],
            redirections: vec![
                Redirection {
                    redirected_fd: 1,
                    operator: RedirectOperator::OutputTruncate,
                    operand: Word::from_parts(vec![WordPart::Unquoted("2".into())]),
                },
                Redirection {
                    redirected_fd: 2,
                    operator: RedirectOperator::DuplicateOutput,
                    operand: Word::from_parts(vec![WordPart::Unquoted("1".into())]),
                },
            ],
        };

        let expanded = expander()
            .expand_command(command)
            .expect("redirection operands should resolve");

        assert_eq!(
            expanded.redirections[0].operand,
            ExpandedRedirectOperand::Path("2".into())
        );
        assert_eq!(
            expanded.redirections[1].operand,
            ExpandedRedirectOperand::Fd(1)
        );
    }

    #[test]
    fn rejects_a_non_numeric_file_descriptor_after_expansion() {
        let command = Command {
            args: vec![],
            redirections: vec![Redirection {
                redirected_fd: 2,
                operator: RedirectOperator::DuplicateOutput,
                operand: Word::from_parts(vec![WordPart::Unquoted("stdout".into())]),
            }],
        };

        assert_eq!(
            expander().expand_command(command).map(|_| ()),
            Err(super::ExpanderError::InvalidFileDescriptor("stdout".into()))
        );
    }
}
