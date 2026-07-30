use std::{iter::Peekable, mem, str::Chars};
use thiserror::Error;

use crate::token::{
    RedirectOperator::{DuplicateInput, DuplicateOutput, Input, OutputAppend, OutputTruncate},
    Token, Word, WordPart,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexerState {
    Normal,
    InSingleQuote,
    InDoubleQuote,
}

pub struct Lexer<'src> {
    // 解析后产生的Token数组
    tokens: Vec<Token>,
    // 转为peekable迭代器让lexer可以偷看下一个元素
    source: Peekable<Chars<'src>>,
    // 当前正在处理的Word
    current_word: Option<WordBuilder>,
    // Lexer状态机状态栈
    state: LexerStateManager,
}

impl<'src> Lexer<'src> {
    pub fn new(source: &'src str) -> Self {
        Self {
            tokens: Vec::new(),
            source: source.chars().peekable(),
            current_word: None,
            state: LexerStateManager::new(),
        }
    }

    pub fn lex(&mut self) -> Result<Vec<Token>, LexerError> {
        while let Some(this_char) = self.source.next() {
            match self.state.current()? {
                LexerState::Normal => self.lex_normal(this_char)?,
                LexerState::InSingleQuote => self.lex_single_quoted(this_char)?,
                LexerState::InDoubleQuote => self.lex_double_quoted(this_char)?,
            }
        }

        self.finish_word();
        
        match self.state.current()? {
            LexerState::InDoubleQuote => Err(LexerError::UnclosedDoubleQuote),
            LexerState::InSingleQuote => Err(LexerError::UnclosedSingleQuote),
            LexerState::Normal => Ok(self.tokens.clone()), // TODO: 用mem::take
        }
    }

    fn lex_normal(&mut self, this_char: char) -> Result<(), LexerError> {
        match this_char {
            '\'' => {
                self.current_word
                    .get_or_insert_default()
                    .finish_unquoted_part(); // 先提交未提交的Unquoted部分
                self.state.push(LexerState::InSingleQuote)?;
            }
            '\"' => {
                self.current_word
                    .get_or_insert_default()
                    .finish_unquoted_part(); // 先提交未提交的Unquoted部分
                self.state.push(LexerState::InDoubleQuote)?;
            }
            '>' | '<' => {
                let next_char = self.source.peek().copied();
                // ! 此处用take将current消费掉
                self.finish_before_redirect()?;

                let token = match (this_char, next_char) {
                    ('>', Some('>')) => {
                        // 消费下一个char
                        self.source.next();

                        // >> 追加写入
                        Token::Redirect(OutputAppend)
                    }
                    ('>', Some('&')) => {
                        // 消费下一个char
                        self.source.next();
                        // 文件描述符复制或关闭
                        Token::Redirect(DuplicateOutput)
                    }
                    ('>', _) => {
                        // > 覆盖写入
                        Token::Redirect(OutputTruncate)
                    }
                    ('<', Some('<')) => {
                        //TODO: Here-doc
                        return Err(LexerError::UnsupportedOperator("<<"));
                    }
                    ('<', Some('&')) => {
                        // 消费下一个char
                        self.source.next();
                        // 输入fd复制
                        Token::Redirect(DuplicateInput)
                    }
                    ('<', _) => {
                        // 读入
                        Token::Redirect(Input)
                    }
                    _ => unreachable!("this branch only handles `<` and `>`"),
                };

                // 重定向操作符右侧始终按 Word 继续词法分析，具体含义在展开后确定。
                self.tokens.push(token);
            }
            '&' => {
                if self.consume_next_if('&') {
                    // AndAnd
                    // 先提交 WordBuild 中未提交的内容
                    self.finish_word();

                    // 然后提交AndAnd
                    self.tokens.push(Token::AndAnd);
                } else {
                    // TODO: Background
                    return Err(LexerError::UnsupportedOperator("&"));
                }
            }
            '|' => {
                if self.consume_next_if('|') {
                    // OrOr
                    self.finish_word();

                    self.tokens.push(Token::OrOr);
                } else {
                    // Pipeline
                    self.finish_word();

                    self.tokens.push(Token::Pipeline);
                }
            }
            '\\' => {
                // Escape
                if let Some(next_char) = self.source.next() {
                    // 转义字符要存在
                    self.current_word
                        .get_or_insert_default()
                        .push_escaped(next_char);
                } else {
                    return Err(LexerError::IncompleteEscape);
                }
            }
            ' ' | '\t' => {
                self.finish_word();
            }
            ch => {
                self.current_word.get_or_insert_default().push_char(ch);
            }
        }
        Ok(())
    }

    fn lex_single_quoted(&mut self, this_char: char) -> Result<(), LexerError> {
        match this_char {
            '\'' => {
                self.current_word
                    .as_mut()
                    .expect("single-quote state must have a current word")
                    .finish_single_quoted_part();

                self.state.pop_quote_state()?;
            }
            ch => {
                self.current_word
                    .as_mut()
                    .expect("single-quote state must have a current word")
                    .push_char(ch);
            }
        }
        Ok(())
    }

    fn lex_double_quoted(&mut self, this_char: char) -> Result<(), LexerError> {
        match this_char {
            '\"' => {
                self.current_word
                    .as_mut()
                    .expect("double-quote state must have a current word")
                    .finish_double_quoted_part();

                self.state.pop_quote_state()?;
            }
            '\\' => {
                let next_char = self.source.peek().copied();
                // 双引号中的转义
                let current_word = self
                    .current_word
                    .as_mut()
                    .expect("double-quote state must have a current word");
                match next_char {
                    Some(escaped_ch @ ('\"' | '\\' | '$' | '`')) => {
                        self.source.next();
                        current_word.push_double_quoted_escaped(escaped_ch);
                    }
                    Some('\n') => {
                        // 行续接, 跳过转义符和换行符
                        self.source.next();
                    }
                    Some(_) | None => {
                        //不存在转义元素或者不可转义，当作字面量处理
                        current_word.push_char('\\');
                    }
                }
            }
            ch => {
                self.current_word
                    .as_mut()
                    .expect("double-quote state must have a current word")
                    .push_char(ch);
            }
        }
        Ok(())
    }

    /// 如果下一个Token是expected则消费并返回true
    ///
    /// # Arguments
    ///
    /// - `expected` (`char`) - 下一个字符的期望值
    ///
    /// # Returns
    ///
    /// - `bool` - 符合期望则返回true, 否则返回false
    fn consume_next_if(&mut self, expected: char) -> bool {
        self.source.next_if_eq(&expected).is_some()
    }

    fn emit_control_operator(&mut self, token: Token) {
        self.finish_word();
        self.tokens.push(token);
    }

    fn finish_word(&mut self) {
        if let Some(builder) = self.current_word.take() {
            self.tokens.push(Token::Word(builder.finish()));
        }
    }

    fn finish_before_redirect(&mut self) -> Result<(), LexerError> {
        if let Some(builder) = self.current_word.take() {
            self.tokens.push(builder.finish_before_redirect()?);
        }
        Ok(())
    }
}

/// 用于构建Word对象的工具类
#[derive(Debug, Default)]
struct WordBuilder {
    parts: Vec<WordPart>, // 解析完成的WordPart
    buffer: String,       // 缓冲区，保存正在解析的部分
}

impl WordBuilder {
    /// 把字符压入WordBuilder缓冲区
    fn push_char(&mut self, c: char) {
        self.buffer.push(c);
    }

    /// 结束一个Unquoted Part
    /// 取出缓冲区内容，创建一个WordPart::Unquoted，放入parts
    fn finish_unquoted_part(&mut self) {
        if self.buffer.is_empty() {
            // 当缓冲区为空的时候不做任何操作
            return;
        }

        self.parts
            .push(WordPart::Unquoted(mem::take(&mut self.buffer)));
    }

    /// 提交一个SingleQuoted Part
    /// 取出缓冲区内容，创建一个WordPart::SingleQuoted，放入parts
    fn finish_single_quoted_part(&mut self) {
        self.parts
            .push(WordPart::SingleQuoted(mem::take(&mut self.buffer)));
    }

    /// 提交一个DoubleQuoted Part
    /// 取出缓冲区内容，创建一个WordPart::DoubleQuoted，放入parts
    fn finish_double_quoted_part(&mut self) {
        self.parts
            .push(WordPart::DoubleQuoted(mem::take(&mut self.buffer)));
    }

    ///在重定向前完成WordBuilder处理,返回Token
    fn finish_before_redirect(mut self) -> Result<Token, LexerError> {
        // 如果有且仅有buffer不为空，且为纯数字，则解析为IONumber
        let is_io_number = self.parts.is_empty()
            && !self.buffer.is_empty()
            && self.buffer.chars().all(|c| c.is_ascii_digit());

        if is_io_number {
            // 未转换类型的 IoNumber，隔离到 text 变量中方便错误处理
            let text = mem::take(&mut self.buffer);
            // 把text转换成u32, 并处理错误
            let fd = text
                .parse::<u32>()
                .map_err(|_| LexerError::InvalidIoNumber(text))?;

            return Ok(Token::IoNumber(fd));
        }

        Ok(Token::Word(self.finish()))
    }

    /// 直接向parts中提交一个 Escaped Part (一个字符)
    fn push_escaped(&mut self, c: char) {
        self.finish_unquoted_part(); // 因为转义符只在非单双引号出现，且为单字符，所以要先清空缓冲区

        self.parts.push(WordPart::Escaped(c));
    }

    fn push_double_quoted_escaped(&mut self, c: char) {
        self.finish_double_quoted_part();

        self.parts.push(WordPart::Escaped(c));
    }

    fn finish(mut self) -> Word {
        self.finish_unquoted_part();

        Word::from_parts(self.parts)
    }
}

struct LexerStateManager {
    state_stack: Vec<LexerState>,
}

impl LexerStateManager {
    fn new() -> Self {
        LexerStateManager {
            state_stack: vec![LexerState::Normal],
        }
    }

    fn push(&mut self, state: LexerState) -> Result<(), LexerStateError> {
        if matches!(state, LexerState::Normal) {
            return Err(LexerStateError::CannotPushBaseState);
        }

        self.state_stack.push(state);
        Ok(())
    }

    fn pop_quote_state(&mut self) -> Result<(), LexerStateError> {
        match self.state_stack.len() {
            0 => Err(LexerStateError::EmptyStack),
            1 => Err(LexerStateError::CannotPopBaseState),
            _ => {
                self.state_stack.pop();
                Ok(())
            }
        }
    }

    fn current(&self) -> Result<LexerState, LexerStateError> {
        self.state_stack.last().ok_or(LexerStateError::EmptyStack)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum LexerStateError {
    #[error("lexer state stack is empty")]
    EmptyStack,
    #[error("cannot push the Normal state onto the lexer state stack")]
    CannotPushBaseState,
    #[error("cannot pop the base Normal state from the lexer state stack")]
    CannotPopBaseState,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum LexerError {
    #[error("invalid lexer state transition: {0}")]
    InvalidStateTransition(#[from] LexerStateError),
    #[error("incomplete escape sequence")]
    IncompleteEscape,
    #[error("unclosed single quote")]
    UnclosedSingleQuote,
    #[error("unclosed double quote")]
    UnclosedDoubleQuote,
    #[error("unsupported operator \'{0}\'")]
    UnsupportedOperator(&'static str),
    #[error("invalid IoNumber: \'{0}\'")]
    // 这里使用String而非u32是因为出错时不能保证String是可以被正常解析成u32的
    InvalidIoNumber(String),
    #[error("unexpected error: {0}")]
    UnexpectedError(String),
}

#[cfg(test)]
mod tests {
    use super::{Lexer, LexerError, LexerState, LexerStateError, LexerStateManager};
    use crate::token::{RedirectOperator, Token, Word, WordPart};

    fn word(parts: Vec<WordPart>) -> Token {
        Token::Word(Word::from_parts(parts))
    }

    #[test]
    fn closing_a_quote_keeps_the_normal_base_state() {
        let mut state = LexerStateManager::new();

        state
            .push(LexerState::InSingleQuote)
            .expect("a quote state can be pushed");
        state
            .pop_quote_state()
            .expect("a quote state can be popped");

        assert!(matches!(state.current(), Ok(LexerState::Normal)));
    }

    #[test]
    fn cannot_pop_the_normal_base_state() {
        let mut state = LexerStateManager::new();

        assert_eq!(
            state.pop_quote_state(),
            Err(LexerStateError::CannotPopBaseState)
        );
        assert!(matches!(state.current(), Ok(LexerState::Normal)));
    }

    #[test]
    fn closing_quotes_does_not_fail_lexing() {
        let lexer = Lexer::new();

        assert!(lexer.lex("echo 'hello'").is_ok());
        assert!(lexer.lex("echo \"hello\"").is_ok());
    }

    #[test]
    fn lexes_word_parts_without_losing_quote_boundaries() {
        let tokens = Lexer::new()
            .lex("echo pre'single'\"double\"\\ value '' \"\"")
            .expect("valid quoting should lex");

        assert_eq!(
            tokens,
            vec![
                word(vec![WordPart::Unquoted("echo".into())]),
                word(vec![
                    WordPart::Unquoted("pre".into()),
                    WordPart::SingleQuoted("single".into()),
                    WordPart::DoubleQuoted("double".into()),
                    WordPart::Escaped(' '),
                    WordPart::Unquoted("value".into()),
                ]),
                word(vec![WordPart::SingleQuoted(String::new())]),
                word(vec![WordPart::DoubleQuoted(String::new())]),
            ]
        );
    }

    #[test]
    fn lexes_redirections_io_numbers_and_and_if() {
        let tokens = Lexer::new()
            .lex("cat 0<input 2>>error && echo done > output")
            .expect("valid operators should lex");

        assert_eq!(
            tokens,
            vec![
                word(vec![WordPart::Unquoted("cat".into())]),
                Token::IoNumber(0),
                Token::Redirect(RedirectOperator::Input),
                word(vec![WordPart::Unquoted("input".into())]),
                Token::IoNumber(2),
                Token::Redirect(RedirectOperator::OutputAppend),
                word(vec![WordPart::Unquoted("error".into())]),
                Token::AndAnd,
                word(vec![WordPart::Unquoted("echo".into())]),
                word(vec![WordPart::Unquoted("done".into())]),
                Token::Redirect(RedirectOperator::OutputTruncate),
                word(vec![WordPart::Unquoted("output".into())]),
            ]
        );
    }

    #[test]
    fn a_quoted_number_before_redirect_is_a_word() {
        let tokens = Lexer::new()
            .lex("'2'>output")
            .expect("quoted number should lex");

        assert_eq!(
            tokens,
            vec![
                word(vec![WordPart::SingleQuoted("2".into())]),
                Token::Redirect(RedirectOperator::OutputTruncate),
                word(vec![WordPart::Unquoted("output".into())]),
            ]
        );
    }

    #[test]
    fn keeps_file_descriptor_duplication_rhs_as_a_word() {
        let tokens = Lexer::new()
            .lex("echo 2>&1 0<&3")
            .expect("file descriptor duplication should lex");

        assert_eq!(
            tokens,
            vec![
                word(vec![WordPart::Unquoted("echo".into())]),
                Token::IoNumber(2),
                Token::Redirect(RedirectOperator::DuplicateOutput),
                word(vec![WordPart::Unquoted("1".into())]),
                Token::IoNumber(0),
                Token::Redirect(RedirectOperator::DuplicateInput),
                word(vec![WordPart::Unquoted("3".into())]),
            ]
        );
    }

    #[test]
    fn double_quotes_only_escape_supported_characters() {
        let tokens = Lexer::new()
            .lex(r#"echo "\$x\q\\\"""#)
            .expect("double-quoted escapes should lex");

        assert_eq!(
            tokens,
            vec![
                word(vec![WordPart::Unquoted("echo".into())]),
                word(vec![
                    WordPart::DoubleQuoted(String::new()),
                    WordPart::Escaped('$'),
                    WordPart::DoubleQuoted(r"x\q".into()),
                    WordPart::Escaped('\\'),
                    WordPart::DoubleQuoted(String::new()),
                    WordPart::Escaped('"'),
                    WordPart::DoubleQuoted(String::new()),
                ]),
            ]
        );
    }

    #[test]
    fn reports_incomplete_constructs_and_unsupported_operators() {
        let lexer = Lexer::new();

        assert_eq!(lexer.lex("echo \\"), Err(LexerError::IncompleteEscape));
        assert_eq!(
            lexer.lex("echo 'open"),
            Err(LexerError::UnclosedSingleQuote)
        );
        assert_eq!(
            lexer.lex("echo \"open"),
            Err(LexerError::UnclosedDoubleQuote)
        );
        assert_eq!(
            lexer.lex("a & b"),
            Err(LexerError::UnsupportedOperator("&"))
        );
        assert_eq!(
            lexer.lex("cat << input"),
            Err(LexerError::UnsupportedOperator("<<"))
        );
    }

    #[test]
    fn reports_an_io_number_that_does_not_fit_u32() {
        assert_eq!(
            Lexer::new().lex("4294967296>output"),
            Err(LexerError::InvalidIoNumber("4294967296".into()))
        );
    }

    #[test]
    fn state_manager_rejects_pushing_another_base_state() {
        let mut state = LexerStateManager::new();

        assert_eq!(
            state.push(LexerState::Normal),
            Err(LexerStateError::CannotPushBaseState)
        );
        assert!(matches!(state.current(), Ok(LexerState::Normal)));
    }
}
