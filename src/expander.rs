use std::{collections::HashMap, env::home_dir};
use thiserror::Error;

use crate::{
    lexer::{RedirectOperator, Word, WordPart},
    parser::Command,
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

pub(crate) enum ExpandedAst {
    Command(ExpandedCommand),
    AndIf {
        left: Box<ExpandedAst>,
        right: Box<ExpandedAst>,
    },
}

pub(crate) struct Expander<'env> {
    env: &'env HashMap<String, String>,
}

impl<'env> Expander<'env> {
    pub(crate) fn new(env: &'env HashMap<String, String>) -> Self {
        Self { env }
    }

    pub(crate) fn expand_command(
        &self,
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

    pub(crate) fn expand_word(&self, word: Word) -> Result<String, ExpanderError> {
        let mut buffer = String::new();
        for (idx, part) in word.into_part().enumerate() {
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
    #[error("Could not found env var `{0}`")]
    EnvironmentVariableNotFound(&'static str),
    #[error("Could not derive tilde, please check environment variables")]
    CouldNotExpandTilde,
}
