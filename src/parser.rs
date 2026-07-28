use std::{iter::Peekable, vec::IntoIter};

use thiserror::Error;

use crate::token::{RedirectOperator, Token, Word};

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Redirection {
    pub(crate) redirected_fd: u32,
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
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum ParserError {
    #[error("unexpected token: `{0:?}`")]
    UnexpectedToken(Token),
    #[error("missing command after &&")]
    MissingCommandAfterAndIf,
    #[error("expect command but get nothing")]
    ExpectCommand,
    #[error("expect redirect operator, but found `{found:?}`")]
    ExpectRedirectOperator { found: Option<Token> },
    #[error("expect redirection operand, but found `{found:?}`")]
    ExpectRedirectionOperand { found: Option<Token> },
}

pub(crate) struct Parser {
    tokens: Peekable<IntoIter<Token>>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens: tokens.into_iter().peekable(),
        }
    }

    // Parser是一次性对象，所有消费自身所有权
    pub fn parse(mut self) -> Result<Option<Ast>, ParserError> {
        // 如果Token为空则直接返回Ok(None)
        if self.tokens.peek().is_none() {
            return Ok(None);
        }

        let ast = self.parse_and_if()?;

        // 检查是否存在parser未消费的剩余Token
        if let Some(token) = self.tokens.next() {
            return Err(ParserError::UnexpectedToken(token));
        }

        Ok(Some(ast))
    }

    fn parse_command(&mut self) -> Result<Ast, ParserError> {
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
                // 当遇到 Token::AndAnd说明当前Command已经结束
                Some(Token::AndAnd) | None => {
                    break;
                }
            }
        }

        // command 为空是合法的, 因为command有可能来源于未解析的重定向输入
        if command.is_empty() && redirections.is_empty() {
            return Err(ParserError::ExpectCommand);
        }

        Ok(Ast::Command(Command {
            args: command,
            redirections,
        }))
    }

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

    fn parse_and_if(&mut self) -> Result<Ast, ParserError> {
        // 先解析左侧命令
        let mut left = self.parse_command()?;

        // 如果发现了Token::AndAnd则递归解析
        while matches!(self.tokens.peek(), Some(Token::AndAnd)) {
            self.tokens.next(); // 消费 Token::AndAnd

            // 检查Token::AndAnd之后是否存在命令
            if self.tokens.peek().is_none() {
                return Err(ParserError::MissingCommandAfterAndIf);
            }

            //解析右侧命令
            let right = self.parse_command()?;
            left = Ast::AndIf {
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
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
        let tokens = Lexer::new().lex(source).expect("test input should lex");
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
}
