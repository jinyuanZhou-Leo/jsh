use std::{iter::Peekable, vec::IntoIter};

use thiserror::Error;

use crate::lexer::{RedirectOperator, Token, Word};

#[derive(Debug, PartialEq, Eq, Clone)]
struct Redirection {
    fd: u32,
    operator: RedirectOperator,
    target: Word
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum Ast {
    Command {
        args: Vec<Word>,
        redirections: Vec<Redirection>,
    },
    AndIf {
        left: Box<Ast>,
        right: Box<Ast>,
    },
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

        Ok(Ast::Command {
            args: command,
            redirections,
        })
    }

    fn parse_redirection(&mut self) -> Result<Redirection, ParserError> {
        let fd = match self.tokens.peek() {
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

        let target = match self.tokens.next() {
            Some(Token::Word(word)) => word,
            Some(token) => {
                return Err(ParserError::ExpectRedirectTarget { found: Some(token) });
            }
            None => {
                return Err(ParserError::ExpectRedirectTarget { found: None });
            }
        };


        Ok(Redirection { fd, operator, target })
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
    #[error("expect redirect target, but found `{found:?}`")]
    ExpectRedirectTarget { found: Option<Token> },
}
