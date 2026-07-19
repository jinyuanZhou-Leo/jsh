use std::{error::Error};

use crate::lexer::{Token, Word, WordPart};

#[derive(Debug, PartialEq, Eq, Clone)]
enum Redirection {
    Input {
        fd: u32,
        target: Word,
    },

    Output {
        fd: u32,
        target: Word,
    },
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum Ast{
    Command{
        args: Vec<Word>,
        redirection: Vec<Redirection>
    },
    AndIf{
        left: Box<Ast>,
        right: Box<Ast>
    }
}

pub struct Parser{
    tokens: Vec<Token>,
    pos: usize
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self{
        Self{tokens, pos: 0}
    }

    pub fn parse(&mut self) -> Result<Ast, Box<dyn Error>> {
        let command = self.parse_command()?;
        Ok(command)
    }

    fn parse_command(&mut self) -> Result<Ast, Box<dyn Error>> {
        let mut command_buf = Vec::with_capacity(2);
        let mut redirection_buf = Vec::with_capacity(2);
        loop {
            match self.peek() {
                Token::Word(word) => {
                    let Token::Word(word) = self.next() else {
                        return Err(format!("Error occurred while parsing word").into());
                    };
                    command_buf.push(word);
                }, 
                Token::IoNumber(_)| Token::RedirectOut | Token::RedirectIn => {
                    self.next(); //消费当前Token
                    let redirection = self.parse_redirection()?;
                    redirection_buf.push(redirection);
                },
                Token::AndIf => {
                    unreachable!();
                }
                Token::Eof => {
                    return Ok(Ast::Command { args: command_buf, redirection: redirection_buf })
                }
                token => {
                    return Err(format!("Unexpected Token, found {token:?}").into())
                }
            }
        }
    }

    fn parse_redirection(&mut self) -> Result<Redirection, Box<dyn Error>> {
        let fd: u32 = match self.current().clone(){
            Token::IoNumber(fd) => {
                self.next(); //消费IoNumber
                fd
            },
            //非IoNumber不消费，只处理
            Token::RedirectIn => 0,
            Token::RedirectOut => 1,
            token => {
                return Err(format!("Expected redirection, but found {token:?}").into());
            }
        };

        let operator = match self.current().clone(){
            Token::RedirectIn => Token::RedirectIn,
            Token::RedirectOut => Token::RedirectOut,
            token => {
                return Err(format!("Expected redirect operator, but found {token:?}").into());
            }
        };

        let target = match self.next(){
            Token::Word(word) => word,
            Token::Eof => {
                return Err(format!("Expected file after redirection").into());
            }
            token => {
                return Err(format!("Expected file, but found {token:?}").into());
            }
        };

        match operator {
            Token::RedirectIn => {
                Ok(Redirection::Input { fd, target })
            },
            Token::RedirectOut => {
                Ok(Redirection::Output { fd, target })
            },
            _ => unreachable!() //SAFTY: operator定义处已经处理过错误，此处不可能出现类型不符合In, Out的问题
        }
    }

    fn parse_and_or(&mut self) -> Result<Ast, Box<dyn Error>>{
        let mut left = self.parse_command()?;

        while matches!(*self.peek(), Token::AndIf) {
            self.next();
            let right = self.parse_command()?;
            left = Ast::AndIf { 
                left: Box::new(left), 
                right: Box::new(right) 
            };
        }

        Ok(left)
    }

    fn advance(&mut self) {
        if !matches!(self.peek(), Token::Eof){
            self.pos += 1;
        }
    }

    fn next(&mut self) -> Token {
        let next_token = self.peek().clone();

        self.advance();

        next_token
    }
    
    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos + 1]
    }
}
