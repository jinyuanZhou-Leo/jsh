use std::{error::Error, mem};

enum LexerState {
    Normal,
    InSingleQuote,
    InDoubleQuote,
}

pub struct Lexer;

impl Lexer {
    pub fn new() -> Self {
        Self {}
    }

    pub fn lex(&self, input: &str) -> Result<Vec<Token>, Box<dyn Error>> {
        let mut tokens: Vec<Token> = Vec::new();
        let mut word_parts: Vec<WordPart> = Vec::new();
        let mut part_text = String::new();
        let mut state_stack = vec![LexerState::Normal];

        for ch in input.chars() {
            match state_stack.last().unwrap() {
                LexerState::Normal => match ch {
                    '\'' => {
                        Self::flush_word_part(
                            &mut word_parts,
                            &mut part_text,
                            WordPartKind::Normal,
                        );
                        state_stack.push(LexerState::InSingleQuote);
                    }
                    '\"' => {
                        Self::flush_word_part(
                            &mut word_parts,
                            &mut part_text,
                            WordPartKind::Normal,
                        );
                        state_stack.push(LexerState::InDoubleQuote);
                    }
                    ' ' => {
                        Self::flush_word_part(
                            &mut word_parts,
                            &mut part_text,
                            WordPartKind::Normal,
                        );
                        Self::flush_word_token(&mut tokens, &mut word_parts);
                    }
                    '>' => {
                        Self::flush_word_part(
                            &mut word_parts,
                            &mut part_text,
                            WordPartKind::Normal,
                        );
                        Self::flush_word_token(&mut tokens, &mut word_parts);
                        tokens.push(Token::Redirect);
                    }
                    ch => {
                        part_text.push(ch); // 正常字符压入缓冲区
                    }
                },
                LexerState::InSingleQuote => match ch {
                    '\'' => {
                        Self::flush_word_part(
                            &mut word_parts,
                            &mut part_text,
                            WordPartKind::SingleQuoted,
                        );
                        state_stack.pop();
                    }
                    ch => {
                        part_text.push(ch);
                    }
                },
                LexerState::InDoubleQuote => match ch {
                    '\"' => {
                        Self::flush_word_part(
                            &mut word_parts,
                            &mut part_text,
                            WordPartKind::DoubleQuoted,
                        );
                        state_stack.pop();
                    }
                    ch => {
                        part_text.push(ch);
                    }
                },
            }
        }

        Self::flush_word_part(&mut word_parts, &mut part_text, WordPartKind::Normal);
        Self::flush_word_token(&mut tokens, &mut word_parts);

        Ok(tokens)
    }

    fn flush_word_part(word_parts: &mut Vec<WordPart>, part_text: &mut String, kind: WordPartKind) {
        if part_text.is_empty() {
            return;
        }

        word_parts.push(kind.into_word_part(mem::take(part_text)));
    }

    fn flush_word_token(tokens: &mut Vec<Token>, word_parts: &mut Vec<WordPart>) {
        if word_parts.is_empty() {
            return;
        }

        tokens.push(Token::Word(mem::take(word_parts)));
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Token {
    Word(Vec<WordPart>),
    Redirect,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WordPart {
    SingleQuoted(String),
    DoubleQuoted(String),
    Normal(String),
}

enum WordPartKind {
    SingleQuoted,
    DoubleQuoted,
    Normal,
}

impl WordPartKind {
    fn into_word_part(&self, content: String) -> WordPart {
        match self {
            Self::SingleQuoted => WordPart::SingleQuoted(content),
            Self::DoubleQuoted => WordPart::DoubleQuoted(content),
            Self::Normal => WordPart::Normal(content),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Lexer, Token, WordPart};

    #[test]
    fn lexes_quoted_and_unquoted_word_parts() {
        let cmd = "echo 'hello        world' -a";
        let lexer = Lexer::new();

        assert_eq!(
            lexer.lex(cmd).unwrap(),
            vec![
                Token::Word(vec![WordPart::Normal("echo".to_string())]),
                Token::Word(vec![WordPart::SingleQuoted(
                    "hello        world".to_string()
                )]),
                Token::Word(vec![WordPart::Normal("-a".to_string())]),
            ]
        );
    }

    #[test]
    fn lexes_redirect_as_token() {
        let lexer = Lexer::new();

        assert_eq!(
            lexer.lex("echo hi > out").unwrap(),
            vec![
                Token::Word(vec![WordPart::Normal("echo".to_string())]),
                Token::Word(vec![WordPart::Normal("hi".to_string())]),
                Token::Redirect,
                Token::Word(vec![WordPart::Normal("out".to_string())]),
            ]
        );
    }
}
