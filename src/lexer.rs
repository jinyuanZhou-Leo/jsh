use std::{error::Error, mem};

enum LexerState {
    Normal,
    InSingleQuote,
    InDoubleQuote,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Token {
    Word(Word),
    RedirectIn,
    RedirectOut,
    IoNumber(u32),
    Eof,
    AndIf,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Word {
    parts: Vec<WordPart>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum WordPart {
    SingleQuoted(String),
    DoubleQuoted(String),
    Normal(String),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
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

        let mut input = input.chars().peekable(); // 使用peekable让lexer可以偷看下一个元素
        while let Some(ch) = input.next() {
            match state_stack.last().unwrap() {
                LexerState::Normal => match ch {
                    ch if part_text.is_empty() && word_parts.is_empty() && ch.is_ascii_digit() => {
                        //当缓冲区为空，且读到数字的时候
                        let Some(next_ch) = input.peek().copied() else {
                            // 如果数字后不存在下一个字符，则正常将该数字放入缓冲区，不做任何特殊处理
                            part_text.push(ch);
                            continue;
                        };

                        if matches!(next_ch, '>' | '<') {
                            //如果紧跟的下一个字符是重定向符号，则特殊处理
                            input.next(); // 移动lexer游标
                            tokens.push(Token::IoNumber(
                                ch.to_digit(10).expect("the current character is a digit"),
                            )); // 把当前字符视为 IO Number
                            match next_ch {
                                '>' => tokens.push(Token::RedirectOut),
                                '<' => tokens.push(Token::RedirectIn),
                                _ => {
                                    unreachable!("next_ch here must be > or <");
                                }
                            }
                        } else {
                            //如果下一个字符不是重定向符号，则正常处理
                            part_text.push(ch);
                        }
                    }
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
                        tokens.push(Token::RedirectOut);
                    }
                    '<' => {
                        Self::flush_word_part(
                            &mut word_parts,
                            &mut part_text,
                            WordPartKind::Normal,
                        );
                        Self::flush_word_token(&mut tokens, &mut word_parts);
                        tokens.push(Token::RedirectIn);
                    }
                    '\\' => {
                        if let Some(next_ch) = input.next() {
                            // TODO: 添加对于特殊转义符的支持
                            //先把缓冲区的内容flush一次
                            Self::flush_word_part(
                                &mut word_parts,
                                &mut part_text,
                                WordPartKind::Normal,
                            );
                            //然后把后一个字符当作字面量(单引号括起来的)处理
                            part_text.push(next_ch);
                            Self::flush_word_part(
                                &mut word_parts,
                                &mut part_text,
                                WordPartKind::SingleQuoted,
                            );
                        } else {
                            //如果下一个字符不存在，说明存在一个非法的转义符
                            return Err("Incomplete escape".into());
                        };
                    }
                    '&' => {
                        let Some(next_ch) = input.peek() else {
                            part_text.push(ch);
                            continue;
                        };

                        if matches!(*next_ch, '&') {
                            input.next(); //消费下一个字符
                            Self::flush_word_part(
                                &mut word_parts,
                                &mut part_text,
                                WordPartKind::Normal,
                            );
                            Self::flush_word_token(&mut tokens, &mut word_parts);
                            tokens.push(Token::AndIf);
                        }
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
                    '\\' => {
                        match input.peek() {
                            Some('\"' | '\\' | '$' | '`' | '\n') => {
                                let next_ch = input.next().unwrap(); // SAFTY: 此处已经通过peek偷看过下一个字符, 该情况中下一个字符一定存在，因此可以安全的unwrap
                                part_text.push(next_ch); // 正常消费下一个元素
                            }
                            Some(c) => {
                                part_text.push('\\'); //下一个元素不能转义，则backslash在此处被当作字面量
                            }
                            None => {
                                part_text.push('\\'); //不存在下一个元素，当作字面量处理
                            }
                        }
                    }
                    ch => {
                        part_text.push(ch);
                    }
                },
            }
        }

        Self::flush_word_part(&mut word_parts, &mut part_text, WordPartKind::Normal);
        Self::flush_word_token(&mut tokens, &mut word_parts);

        if !matches!(state_stack.last(), Some(LexerState::Normal)) {
            return Err("Unclosed quote".into());
        }

        tokens.push(Token::Eof); // 插入Eof表示结束

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

        tokens.push(Token::Word(Word {
            parts: mem::take(word_parts),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::{Lexer, Token, Word, WordPart};

    fn word(parts: Vec<WordPart>) -> Token {
        Token::Word(Word { parts })
    }

    fn lex(input: &str) -> Vec<Token> {
        Lexer::new()
            .lex(input)
            .expect("input should lex successfully")
    }

    #[test]
    fn lexes_quoted_and_unquoted_word_parts() {
        let cmd = "echo 'hello        world' -a";
        let lexer = Lexer::new();

        assert_eq!(
            lexer.lex(cmd).unwrap(),
            vec![
                word(vec![WordPart::Normal("echo".to_string())]),
                word(vec![WordPart::SingleQuoted(
                    "hello        world".to_string()
                )]),
                word(vec![WordPart::Normal("-a".to_string())]),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn lexes_input_and_output_redirects_with_io_number() {
        assert_eq!(
            lex("cat<in 2>out"),
            vec![
                word(vec![WordPart::Normal("cat".to_string())]),
                Token::RedirectIn,
                word(vec![WordPart::Normal("in".to_string())]),
                Token::IoNumber(2),
                Token::RedirectOut,
                word(vec![WordPart::Normal("out".to_string())]),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn lexes_adjacent_quoted_and_unquoted_parts_as_one_word() {
        assert_eq!(
            lex(r#"prefix'single'"double""#),
            vec![
                word(vec![
                    WordPart::Normal("prefix".to_string()),
                    WordPart::SingleQuoted("single".to_string()),
                    WordPart::DoubleQuoted("double".to_string()),
                ]),
                Token::Eof
            ]
        );
    }

    #[test]
    fn lexes_and_if_operator() {
        assert_eq!(
            lex("true && echo ok"),
            vec![
                word(vec![WordPart::Normal("true".to_string())]),
                Token::AndIf,
                word(vec![WordPart::Normal("echo".to_string())]),
                word(vec![WordPart::Normal("ok".to_string())]),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn lexes_unquoted_escape_as_literal_word_part() {
        assert_eq!(
            lex(r#"echo hello\ world"#),
            vec![
                word(vec![WordPart::Normal("echo".to_string())]),
                word(vec![
                    WordPart::Normal("hello".to_string()),
                    WordPart::SingleQuoted(" ".to_string()),
                    WordPart::Normal("world".to_string()),
                ]),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn lexes_supported_double_quoted_escapes() {
        assert_eq!(
            lex(r#"echo "a\"b\\c\$d""#),
            vec![
                word(vec![WordPart::Normal("echo".to_string())]),
                word(vec![WordPart::DoubleQuoted("a\"b\\c$d".to_string())]),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn lexes_empty_input_as_eof() {
        assert_eq!(lex(""), vec![Token::Eof]);
    }

    #[test]
    fn reports_incomplete_escape() {
        let error = Lexer::new().lex("echo \\").unwrap_err();

        assert_eq!(error.to_string(), "Incomplete escape");
    }

    #[test]
    fn reports_unclosed_quote() {
        let error = Lexer::new().lex("echo 'unterminated").unwrap_err();

        assert_eq!(error.to_string(), "Unclosed quote");
    }
}
