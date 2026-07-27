use std::{collections::HashMap, env::home_dir};
use thiserror::Error;

use crate::{
    parser::Command,
    token::{RedirectOperator, Word, WordPart},
};

pub(crate) struct ExpandedRedirection {
    pub(crate) fd: u32,
    pub(crate) operator: RedirectOperator,
    pub(crate) target: String,
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
    pub(crate) fn new(env: &'env HashMap<String, String>) -> Self {
        Self { env }
    }

    /// 展开Command类型
    pub(crate) fn expand_command(
        self,
        command: Command,
    ) -> Result<ExpandedCommand, ExpanderError> {
        let Command { args, redirections } = command;

        // 展开过程不会改变数组长度，使用with_capacity减少不必要的堆内存分配
        let mut expanded_args = Vec::with_capacity(args.len());
        let mut expanded_redirections = Vec::with_capacity(redirections.len());

        for word in args {
            expanded_args.push(self.expand_word(word)?);
        }

        for redirection in redirections {
            expanded_redirections.push(ExpandedRedirection {
                fd: redirection.fd,
                operator: redirection.operator,
                target: self.expand_word(redirection.target)?,
            });
        }

        Ok(ExpandedCommand {
            args: expanded_args,
            redirections: expanded_redirections,
        })
    }

    /// 展开Word中所有的WordPart, 返回String
    pub(crate) fn expand_word(&self, word: Word) -> Result<String, ExpanderError> {
        let mut buffer = String::new();
        for (idx, part) in word.into_parts().enumerate() {
            match part {
                WordPart::Unquoted(content) => {
                    if idx == 0 && content.starts_with('~') {
                        // 对词首第一个以~开头的WordPart做展开
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

    fn expand_tilde(&self, input: &str) -> Result<String, ExpanderError> {
        // 使用env库获取home_dir以提供多平台的支持
        let home_dir = home_dir().ok_or(ExpanderError::CouldNotExpandTilde)?;

        let home_path = home_dir
            .to_str()
            .ok_or(ExpanderError::CouldNotExpandTilde)?;

        Ok(input.replace('~', home_path))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum ExpanderError {
    // #[error("Could not found env var `{0}`")]
    // EnvironmentVariableNotFound(&'static str),
    #[error("Could not derive tilde, please check environment variables")]
    CouldNotExpandTilde,
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, env::home_dir, sync::OnceLock};

    use super::Expander;
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

        assert_eq!(expander().expand_word(quoted), Ok("~/project".into()));
        assert_eq!(
            expander().expand_word(non_initial),
            Ok("prefix~/project".into())
        );
    }

    #[test]
    fn expands_command_arguments_and_redirection_targets() {
        let command = Command {
            args: vec![
                Word::from_parts(vec![WordPart::Unquoted("echo".into())]),
                Word::from_parts(vec![WordPart::SingleQuoted("hello world".into())]),
            ],
            redirections: vec![Redirection {
                fd: 2,
                operator: RedirectOperator::OutputAppend,
                target: Word::from_parts(vec![WordPart::Unquoted("error.log".into())]),
            }],
        };

        let expanded = expander()
            .expand_command(command)
            .expect("command should expand");

        assert_eq!(expanded.args, vec!["echo", "hello world"]);
        assert_eq!(expanded.redirections.len(), 1);
        assert_eq!(expanded.redirections[0].fd, 2);
        assert_eq!(
            expanded.redirections[0].operator,
            RedirectOperator::OutputAppend
        );
        assert_eq!(expanded.redirections[0].target, "error.log");
    }
}
