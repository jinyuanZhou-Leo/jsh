use std::{iter::Peekable, vec::IntoIter};

use thiserror::Error;

use crate::token::{RedirectOperator, Token, Word};

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Redirection {
    pub(crate) redirected_fd: i32,
    pub(crate) operator: RedirectOperator,
    /// 操作符右侧尚未展开的操作数。
    pub(crate) operand: Word,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Command {
    pub(crate) args: Vec<Word>,
    pub(crate) redirections: Vec<Redirection>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum Ast {
    Command(Command),
    AndIf { left: Box<Ast>, right: Box<Ast> },
    OrIf { left: Box<Ast>, right: Box<Ast> },
    Seq(Vec<Ast>),
    Pipeline { commands: Vec<Command> },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum ParserError {
    #[error("unexpected token: `{0:?}`")]
    UnexpectedToken(Token),
    #[error("missing command after &&")]
    MissingCommandAfterAndIf,
    #[error("missing command after ||")]
    MissingCommandAfterOrIf,
    #[error("expect command but get nothing")]
    ExpectCommand,
    #[error("expect redirect operator, but found `{found:?}`")]
    ExpectRedirectOperator { found: Option<Token> },
    #[error("expect redirection operand, but found `{found:?}`")]
    ExpectRedirectionOperand { found: Option<Token> },
    #[error("Unexpected error occured while parsing input")]
    UnexpectedError
}

pub(crate) struct Parser {
    tokens: Peekable<IntoIter<Token>>,
}

impl Parser {
    /// 使用有序 Token 序列创建一次性 Parser。
    ///
    /// # Arguments
    ///
    /// * `tokens` - Lexer 生成的 Token 序列。
    ///
    /// # Returns
    ///
    /// 尚未消费任何 Token 的 [`Parser`]。
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens: tokens.into_iter().peekable(),
        }
    }

    /// 消费 Parser 并生成完整的抽象语法树。
    ///
    /// # Returns
    ///
    /// 空 Token 序列返回 [`None`]，否则返回解析完成的 [`Ast`]。
    ///
    /// # Errors
    ///
    /// 输入包含意外 Token、不完整重定向或控制操作符缺少命令时返回 [`ParserError`]。
    pub fn parse(mut self) -> Result<Option<Ast>, ParserError> {
        // 如果Token为空则直接返回Ok(None)
        if self.tokens.peek().is_none() {
            return Ok(None);
        }

        let ast = self.parse_sequence()?;

        // 检查是否存在parser未消费的剩余Token
        if let Some(token) = self.tokens.next() {
            return Err(ParserError::UnexpectedToken(token));
        }

        Ok(Some(ast))
    }

    /// 解析一条命令及其按源码顺序排列的重定向。
    ///
    /// # Returns
    ///
    /// 解析完成的 [`Command`]。
    ///
    /// # Errors
    ///
    /// 当前不存在可构成命令的 Token，或重定向不完整时返回 [`ParserError`]。
    fn parse_command(&mut self) -> Result<Command, ParserError> {
        let mut command = Vec::new();
        let mut redirections = Vec::new();

        loop {
            match self.tokens.peek() {
                Some(Token::Word(_)) => {
                    let Some(Token::Word(word)) = self.tokens.next() else {
                        unreachable!("peek confirmed that the next token exists")
                    };
                    command.push(word);
                }
                Some(Token::IoNumber(_) | Token::Redirect(_)) => {
                    redirections.push(self.parse_redirection()?);
                }
                // 当遇到这些说明当前Command已经结束
                Some(Token::AndAnd) | Some(Token::OrOr) | Some(Token::Pipeline) | Some(Token::Semicolon) | None => {
                    break;
                }
            }
        }

        // command 为空是合法的, 因为command有可能来源于未解析的重定向输入
        if command.is_empty() && redirections.is_empty() {
            return Err(ParserError::ExpectCommand);
        }

        Ok(Command {
            args: command,
            redirections,
        })
    }

    /// 解析可选文件描述符、重定向操作符及其 Word 操作数。
    ///
    /// # Returns
    ///
    /// 保留尚未展开操作数的 [`Redirection`]。
    ///
    /// # Errors
    ///
    /// 缺少重定向操作符或操作数时返回 [`ParserError`]。
    fn parse_redirection(&mut self) -> Result<Redirection, ParserError> {
        let redirected_fd = match self.tokens.peek() {
            Some(Token::IoNumber(_)) => {
                let Some(Token::IoNumber(fd)) = self.tokens.next() else {
                    unreachable!("peek confirmed that the next token exists")
                };

                fd
            }
            Some(Token::Redirect(operator)) => operator.default_fd(),
            Some(_) | None => {
                unreachable!(
                    "peek confirmed that the next token exists and must be either IoNumber or Redirect"
                );
            }
        };

        let operator = match self.tokens.next() {
            Some(Token::Redirect(operator)) => operator,
            Some(token) => {
                return Err(ParserError::ExpectRedirectOperator { found: Some(token) });
            }
            None => {
                return Err(ParserError::ExpectRedirectOperator { found: None });
            }
        };

        let operand = match self.tokens.next() {
            Some(Token::Word(word)) => word,
            Some(token) => {
                return Err(ParserError::ExpectRedirectionOperand { found: Some(token) });
            }
            None => {
                return Err(ParserError::ExpectRedirectionOperand { found: None });
            }
        };

        Ok(Redirection {
            redirected_fd,
            operator,
            operand,
        })
    }

    /// 按左结合规则解析由 `&&` 和 `||` 连接的表达式。
    ///
    /// # Returns
    ///
    /// 条件操作符组合后的 [`Ast`]。
    ///
    /// # Errors
    ///
    /// 条件操作符任一侧缺少命令，或子表达式解析失败时返回 [`ParserError`]。
    fn parse_and_or(&mut self) -> Result<Ast, ParserError> {
        // 先解析左侧命令
        let mut left = self.parse_pipeline()?;

        loop {
            if self.tokens.next_if_eq(&Token::AndAnd).is_some() {
                if self.tokens.peek().is_none() {
                    return Err(ParserError::MissingCommandAfterAndIf);
                }

                let right = self.parse_pipeline()?;
                left = Ast::AndIf {
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if self.tokens.next_if_eq(&Token::OrOr).is_some() {
                if self.tokens.peek().is_none() {
                    return Err(ParserError::MissingCommandAfterOrIf);
                }

                let right = self.parse_pipeline()?;
                left = Ast::OrIf {
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }

        Ok(left)
    }

    /// 解析优先级高于条件操作符的管道表达式。
    ///
    /// # Returns
    ///
    /// 单条命令返回 [`Ast::Command`]，包含管道操作符时返回 [`Ast::Pipeline`]。
    ///
    /// # Errors
    ///
    /// 管道操作符任一侧缺少命令，或命令解析失败时返回 [`ParserError`]。
    fn parse_pipeline(&mut self) -> Result<Ast, ParserError> {
        let first = self.parse_command()?;

        if self.tokens.next_if_eq(&Token::Pipeline).is_none() {
            return Ok(Ast::Command(first));
        }

        let mut commands = vec![first];
        loop {
            commands.push(self.parse_command()?);
            if self.tokens.next_if_eq(&Token::Pipeline).is_none() {
                break;
            }
        }

        Ok(Ast::Pipeline { commands })
    }

    /// 解析由 `;` 构成的命令序列
    /// 
    /// # Returns
    /// 
    /// 单条命令返回值取决于 [`Parser::parse_and_or`] 的返回值，多条命令组成的Sequence返回 [`Ast::Seq`]。
    /// 
    /// # Errors
    /// 
    /// 当 Seq 长度为0时返回 [`ParserError`]
    fn parse_sequence(&mut self) -> Result<Ast, ParserError> {
        let first = self.parse_and_or()?;
        let mut sequence = vec![first];
        loop {
            if self.tokens.next_if_eq(&Token::Semicolon).is_some(){
                // TODO: 对照POSIX文档，检查分号后无命令是否为非法行为
                sequence.push(self.parse_and_or()?);
            } else {
                break;
            }
        }

        if sequence.len() > 1 {
            // 序列中有多个命令
            Ok(Ast::Seq(sequence))
        } else if !sequence.is_empty() {
            // 如果序列中只有一个命令，则不要用Seq封装一层，直接展平
            Ok(sequence.pop().unwrap())
        } else {
            // 理论上 unreachable!
            Err(ParserError::UnexpectedError)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Ast, Command, Parser, ParserError, Redirection};
    use crate::{
        lexer::Lexer,
        token::{RedirectOperator, Token, Word, WordPart},
    };

    fn word(value: &str) -> Word {
        Word::from_parts(vec![WordPart::Unquoted(value.into())])
    }

    fn parse(source: &str) -> Result<Option<Ast>, ParserError> {
        let tokens = Lexer::new(source).lex().expect("test input should lex");
        Parser::new(tokens).parse()
    }

    #[test]
    fn parses_empty_input_as_no_ast() {
        assert_eq!(parse(""), Ok(None));
        assert_eq!(parse(" \t "), Ok(None));
    }

    #[test]
    fn parses_command_arguments_and_ordered_redirections() {
        let ast = parse("cat input 2>>error >output <source")
            .expect("valid command should parse")
            .expect("command should produce an AST");

        assert_eq!(
            ast,
            Ast::Command(Command {
                args: vec![word("cat"), word("input")],
                redirections: vec![
                    Redirection {
                        redirected_fd: 2,
                        operator: RedirectOperator::OutputAppend,
                        operand: word("error"),
                    },
                    Redirection {
                        redirected_fd: 1,
                        operator: RedirectOperator::OutputTruncate,
                        operand: word("output"),
                    },
                    Redirection {
                        redirected_fd: 0,
                        operator: RedirectOperator::Input,
                        operand: word("source"),
                    },
                ],
            })
        );
    }

    #[test]
    fn permits_a_redirection_only_command() {
        assert_eq!(
            parse(">output"),
            Ok(Some(Ast::Command(Command {
                args: vec![],
                redirections: vec![Redirection {
                    redirected_fd: 1,
                    operator: RedirectOperator::OutputTruncate,
                    operand: word("output"),
                }],
            })))
        );
    }

    #[test]
    fn keeps_redirection_operand_unexpanded() {
        assert_eq!(
            parse("echo 2>&1"),
            Ok(Some(Ast::Command(Command {
                args: vec![word("echo")],
                redirections: vec![Redirection {
                    redirected_fd: 2,
                    operator: RedirectOperator::DuplicateOutput,
                    operand: word("1"),
                }],
            })))
        );
    }

    #[test]
    fn parses_and_if_left_associatively() {
        let command = |name| {
            Ast::Command(Command {
                args: vec![word(name)],
                redirections: vec![],
            })
        };

        assert_eq!(
            parse("first && second && third"),
            Ok(Some(Ast::AndIf {
                left: Box::new(Ast::AndIf {
                    left: Box::new(command("first")),
                    right: Box::new(command("second")),
                }),
                right: Box::new(command("third")),
            }))
        );
    }

    #[test]
    fn parses_or_if_left_associatively() {
        let command = |name| {
            Ast::Command(Command {
                args: vec![word(name)],
                redirections: vec![],
            })
        };

        assert_eq!(
            parse("first || second || third"),
            Ok(Some(Ast::OrIf {
                left: Box::new(Ast::OrIf {
                    left: Box::new(command("first")),
                    right: Box::new(command("second")),
                }),
                right: Box::new(command("third")),
            }))
        );
    }

    #[test]
    fn parses_pipeline_commands_in_order() {
        assert_eq!(
            parse("first | second arg | third"),
            Ok(Some(Ast::Pipeline {
                commands: vec![
                    Command {
                        args: vec![word("first")],
                        redirections: vec![],
                    },
                    Command {
                        args: vec![word("second"), word("arg")],
                        redirections: vec![],
                    },
                    Command {
                        args: vec![word("third")],
                        redirections: vec![],
                    },
                ],
            }))
        );
    }

    #[test]
    fn pipeline_binds_tighter_than_and_or() {
        let command = |name| Command {
            args: vec![word(name)],
            redirections: vec![],
        };

        assert_eq!(
            parse("first | second || third && fourth | fifth"),
            Ok(Some(Ast::AndIf {
                left: Box::new(Ast::OrIf {
                    left: Box::new(Ast::Pipeline {
                        commands: vec![command("first"), command("second")],
                    }),
                    right: Box::new(Ast::Command(command("third"))),
                }),
                right: Box::new(Ast::Pipeline {
                    commands: vec![command("fourth"), command("fifth")],
                }),
            }))
        );
    }

    #[test]
    fn rejects_incomplete_redirections() {
        assert_eq!(
            Parser::new(vec![Token::IoNumber(2)]).parse(),
            Err(ParserError::ExpectRedirectOperator { found: None })
        );
        assert_eq!(
            Parser::new(vec![
                Token::IoNumber(2),
                Token::Word(word("not-an-operator")),
            ])
            .parse(),
            Err(ParserError::ExpectRedirectOperator {
                found: Some(Token::Word(word("not-an-operator"))),
            })
        );
        assert_eq!(
            parse("echo >"),
            Err(ParserError::ExpectRedirectionOperand { found: None })
        );
        assert_eq!(
            Parser::new(vec![
                Token::Redirect(RedirectOperator::OutputTruncate),
                Token::AndAnd,
            ])
            .parse(),
            Err(ParserError::ExpectRedirectionOperand {
                found: Some(Token::AndAnd),
            })
        );
    }

    #[test]
    fn rejects_missing_commands_around_and_if() {
        assert_eq!(parse("&& echo ok"), Err(ParserError::ExpectCommand));
        assert_eq!(
            parse("echo ok &&"),
            Err(ParserError::MissingCommandAfterAndIf)
        );
    }

    #[test]
    fn rejects_missing_commands_around_or_if() {
        assert_eq!(parse("|| echo ok"), Err(ParserError::ExpectCommand));
        assert_eq!(
            parse("echo ok ||"),
            Err(ParserError::MissingCommandAfterOrIf)
        );
    }

    #[test]
    fn rejects_missing_commands_around_pipeline() {
        assert_eq!(parse("| echo ok"), Err(ParserError::ExpectCommand));
        assert_eq!(parse("echo ok |"), Err(ParserError::ExpectCommand));
        assert_eq!(parse("echo ok | | next"), Err(ParserError::ExpectCommand));
    }
}
