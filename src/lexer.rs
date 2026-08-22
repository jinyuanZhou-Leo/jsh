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
    /// 创建一次性词法分析器，并借用待分析的源文本。
    ///
    /// 每个 `Lexer` 实例只负责一段源文本；调用 [`Lexer::lex`] 后实例会被消费。
    ///
    /// # Arguments
    ///
    /// * `source` - 待分析的 Shell 源文本。
    ///
    /// # Returns
    ///
    /// 状态处于 `Normal` 且尚未产生 Token 的 [`Lexer`]。
    pub fn new(source: &'src str) -> Self {
        Self {
            tokens: Vec::new(),
            source: source.chars().peekable(),
            current_word: None,
            state: LexerStateManager::new(),
        }
    }

    /// 将源文本转换为有序的 Token 序列。
    ///
    /// 该方法按照当前状态分派字符，保留引号和转义形成的 [`WordPart`] 边界，
    /// 并在重定向操作符前识别紧邻的文件描述符数字。
    ///
    /// # Returns
    ///
    /// 按源码顺序排列的 [`Token`] 序列。
    ///
    /// # Errors
    ///
    /// 当输入包含未闭合引号、不完整转义、不支持的操作符、无效文件描述符，
    /// 或 Lexer 状态栈违反内部不变量时返回 [`LexerError`]。
    pub fn lex(mut self) -> Result<Vec<Token>, LexerError> {
        while let Some(this_char) = self.source.next() {
            match self.state.current()? {
                LexerState::Normal => self.lex_normal(this_char)?,
                LexerState::InSingleQuote => self.lex_single_quoted(this_char)?,
                LexerState::InDoubleQuote => self.lex_double_quoted(this_char)?,
            }
        }

        match self.state.current()? {
            LexerState::InDoubleQuote => Err(LexerError::UnclosedDoubleQuote),
            LexerState::InSingleQuote => Err(LexerError::UnclosedSingleQuote),
            LexerState::Normal => {
                self.finish_word();
                Ok(self.tokens)
            }
        }
    }

    /// 处理 `Normal` 状态下已经从输入流取出的一个字符。
    ///
    /// 复合操作符和转义序列可能会额外消费一个后继字符。
    ///
    /// # Arguments
    ///
    /// * `this_char` - 当前已经从输入流消费的字符。
    ///
    /// # Errors
    ///
    /// 输入包含不完整转义、不支持的操作符、无效文件描述符，或状态转换失败时
    /// 返回 [`LexerError`]。
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
                self.finish_before_redirect()?;

                let token = match this_char {
                    '>' if self.consume_next_if('>') => {
                        // >> 追加写入
                        Token::Redirect(OutputAppend)
                    }
                    '>' if self.consume_next_if('&') => {
                        // 文件描述符复制或关闭
                        Token::Redirect(DuplicateOutput)
                    }
                    '>' => {
                        // > 覆盖写入
                        Token::Redirect(OutputTruncate)
                    }
                    '<' if self.consume_next_if('<') => {
                        //TODO: Here-doc
                        return Err(LexerError::UnsupportedOperator("<<"));
                    }
                    '<' if self.consume_next_if('&') => {
                        // 输入fd复制
                        Token::Redirect(DuplicateInput)
                    }
                    '<' => {
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
                    self.emit_control_operator(Token::AndAnd);
                } else {
                    // TODO: Background
                    return Err(LexerError::UnsupportedOperator("&"));
                }
            }
            '|' => {
                if self.consume_next_if('|') {
                    // OrOr
                    self.emit_control_operator(Token::OrOr);
                } else {
                    // Pipeline
                    self.emit_control_operator(Token::Pipeline);
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
            },
            ';' => {
                self.emit_control_operator(Token::Semicolon);
            },
            ' ' | '\t' => {
                self.finish_word();
            }
            ch => {
                self.current_word.get_or_insert_default().push_char(ch);
            }
        }
        Ok(())
    }

    /// 处理单引号状态下的一个字符，并在遇到闭合单引号时提交 quoted part。
    ///
    /// 空单引号仍会产生一个空的 [`WordPart::SingleQuoted`]，以保留空参数语义。
    ///
    /// # Arguments
    ///
    /// * `this_char` - 当前已经从输入流消费的字符。
    ///
    /// # Errors
    ///
    /// 闭合引号导致状态栈转换失败时返回 [`LexerError`]。
    ///
    /// # Panics
    ///
    /// Lexer 处于单引号状态但不存在当前 Word 时触发 panic；正常状态转换会保持该不变量。
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

    /// 处理双引号状态下的一个字符。
    ///
    /// 双引号内仅 `"`、`\\`、`$` 和反引号可以被反斜杠转义；反斜杠加换行
    /// 被作为行续接丢弃，其他反斜杠按字面量保留。
    ///
    /// # Arguments
    ///
    /// * `this_char` - 当前已经从输入流消费的字符。
    ///
    /// # Errors
    ///
    /// 闭合引号导致状态栈转换失败时返回 [`LexerError`]。
    ///
    /// # Panics
    ///
    /// Lexer 处于双引号状态但不存在当前 Word 时触发 panic；正常状态转换会保持该不变量。
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

    /// 如果输入流的下一个字符等于 `expected`，则消费该字符并返回 `true`。
    ///
    /// 不匹配或输入已经结束时返回 `false`，且不会推进输入流。
    ///
    /// # Arguments
    ///
    /// * `expected` - 期望匹配并消费的字符。
    ///
    /// # Returns
    ///
    /// 下一个字符匹配并已消费时返回 `true`，否则返回 `false`。
    fn consume_next_if(&mut self, expected: char) -> bool {
        self.source.next_if_eq(&expected).is_some()
    }

    /// 结束当前 Word，并提交一个控制操作符 Token。
    ///
    /// 注意: 该方法会结束当前正在构建的Word, 不应当在该方法前调用 [`Lexer::finish_word`] 方法！
    /// [`Lexer::finish_before_redirect`] 保留 IO number 识别语义。
    ///
    /// # Arguments
    ///
    /// * `token` - 待提交的控制操作符 Token。
    fn emit_control_operator(&mut self, token: Token) {
        self.finish_word();
        self.tokens.push(token);
    }

    /// 如果当前正在构建 Word，则结束构建并提交 [`Token::Word`]。
    ///
    /// 尚未开始 Word 时不产生任何 Token。
    fn finish_word(&mut self) {
        if let Some(builder) = self.current_word.take() {
            self.tokens.push(Token::Word(builder.finish()));
        }
    }

    /// 在重定向操作符前结束当前 Word，并提交 Word 或 IO number Token。
    ///
    /// 只有完全由未引用 ASCII 数字组成、且紧邻重定向操作符的内容才会成为
    /// [`Token::IoNumber`]；其他内容仍作为 [`Token::Word`] 提交。
    ///
    /// # Errors
    ///
    /// 当数字超出 `i32` 范围时返回 [`LexerError::InvalidIoNumber`]。
    fn finish_before_redirect(&mut self) -> Result<(), LexerError> {
        if let Some(builder) = self.current_word.take() {
            self.tokens.push(builder.finish_before_redirect()?);
        }
        Ok(())
    }
}

/// 增量构建一个 [`Word`]，并保留未引用、引用和转义部分的边界。
#[derive(Debug, Default)]
struct WordBuilder {
    parts: Vec<WordPart>, // 解析完成的WordPart
    buffer: String,       // 缓冲区，保存正在解析的部分
}

impl WordBuilder {
    /// 将一个普通字符追加到当前尚未提交的 part 缓冲区。
    ///
    /// # Arguments
    ///
    /// * `c` - 需要追加的字符。
    fn push_char(&mut self, c: char) {
        self.buffer.push(c);
    }

    /// 提交当前非空缓冲区为 [`WordPart::Unquoted`]。
    ///
    /// 空缓冲区不会产生 part，因为未引用的空内容不代表一个 shell 参数。
    fn finish_unquoted_part(&mut self) {
        if self.buffer.is_empty() {
            // 当缓冲区为空的时候不做任何操作
            return;
        }

        self.parts
            .push(WordPart::Unquoted(mem::take(&mut self.buffer)));
    }

    /// 提交当前缓冲区为 [`WordPart::SingleQuoted`]。
    ///
    /// 即使缓冲区为空也会提交，以保留 `''` 表示空参数的语义。
    fn finish_single_quoted_part(&mut self) {
        self.parts
            .push(WordPart::SingleQuoted(mem::take(&mut self.buffer)));
    }

    /// 提交当前缓冲区为 [`WordPart::DoubleQuoted`]。
    ///
    /// 即使缓冲区为空也会提交，以保留 `""` 表示空参数的语义。
    fn finish_double_quoted_part(&mut self) {
        self.parts
            .push(WordPart::DoubleQuoted(mem::take(&mut self.buffer)));
    }

    /// 在重定向操作符前结束构建，并将结果分类为 IO number 或普通 Word。
    ///
    /// 只有尚未产生 quoted/escaped part 且缓冲区为纯 ASCII 数字时，结果才是
    /// [`Token::IoNumber`]。
    ///
    /// # Returns
    ///
    /// 纯数字未引用内容返回 [`Token::IoNumber`]，其他内容返回 [`Token::Word`]。
    ///
    /// # Errors
    ///
    /// 当纯数字内容不能表示为 `i32` 时返回 [`LexerError::InvalidIoNumber`]。
    fn finish_before_redirect(mut self) -> Result<Token, LexerError> {
        // 如果有且仅有buffer不为空，且为纯数字，则解析为IONumber
        let is_io_number = self.parts.is_empty()
            && !self.buffer.is_empty()
            && self.buffer.chars().all(|c| c.is_ascii_digit());

        if is_io_number {
            // 未转换类型的 IoNumber，隔离到 text 变量中方便错误处理
            let text = mem::take(&mut self.buffer);
            // 把text转换成i32, 并处理错误
            let fd = text
                .parse::<i32>()
                .map_err(|_| LexerError::InvalidIoNumber(text))?;

            return Ok(Token::IoNumber(fd));
        }

        Ok(Token::Word(self.finish()))
    }

    /// 结束已有的未引用片段，并提交一个受保护的转义字符。
    ///
    /// # Arguments
    ///
    /// * `c` - 反斜杠保护的字符。
    fn push_escaped(&mut self, c: char) {
        self.finish_unquoted_part(); // 因为转义符只在非单双引号出现，且为单字符，所以要先清空缓冲区

        self.parts.push(WordPart::Escaped(c));
    }

    /// 结束已有的双引号片段，并提交一个双引号内的转义字符。
    ///
    /// # Arguments
    ///
    /// * `c` - 双引号内被反斜杠保护的字符。
    fn push_double_quoted_escaped(&mut self, c: char) {
        self.finish_double_quoted_part();

        self.parts.push(WordPart::Escaped(c));
    }

    /// 结束最后一个未引用片段，并消费 builder 生成完整的 [`Word`]。
    ///
    /// # Returns
    ///
    /// 保持所有组成部分顺序的完整 [`Word`]。
    fn finish(mut self) -> Word {
        self.finish_unquoted_part();

        Word::from_parts(self.parts)
    }
}

struct LexerStateManager {
    state_stack: Vec<LexerState>,
}

impl LexerStateManager {
    /// 创建以 [`LexerState::Normal`] 为不可弹出基础状态的状态栈。
    ///
    /// # Returns
    ///
    /// 仅包含基础 `Normal` 状态的管理器。
    fn new() -> Self {
        LexerStateManager {
            state_stack: vec![LexerState::Normal],
        }
    }

    /// 将一个非基础状态压入状态栈。
    ///
    /// # Arguments
    ///
    /// * `state` - 需要压栈的引用状态。
    ///
    /// # Errors
    ///
    /// 传入 [`LexerState::Normal`] 时返回 [`LexerStateError::CannotPushBaseState`]。
    fn push(&mut self, state: LexerState) -> Result<(), LexerStateError> {
        if matches!(state, LexerState::Normal) {
            return Err(LexerStateError::CannotPushBaseState);
        }

        self.state_stack.push(state);
        Ok(())
    }

    /// 弹出当前 quote 状态，同时保证基础 `Normal` 状态始终保留。
    ///
    /// # Errors
    ///
    /// 状态栈为空，或调用将弹出基础状态时返回对应的 [`LexerStateError`]。
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

    /// 返回当前栈顶状态的副本。
    ///
    /// # Returns
    ///
    /// 当前 Lexer 状态。
    ///
    /// # Errors
    ///
    /// 状态栈违反不变量并变为空栈时返回 [`LexerStateError::EmptyStack`]。
    fn current(&self) -> Result<LexerState, LexerStateError> {
        self.state_stack
            .last()
            .ok_or(LexerStateError::EmptyStack)
            .copied()
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
    // 这里使用String而非i32是因为出错时不能保证String是可以被正常解析成i32的
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
        assert!(Lexer::new("echo 'hello'").lex().is_ok());
        assert!(Lexer::new("echo \"hello\"").lex().is_ok());
    }

    #[test]
    fn lexes_word_parts_without_losing_quote_boundaries() {
        let tokens = Lexer::new("echo pre'single'\"double\"\\ value '' \"\"")
            .lex()
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
        let tokens = Lexer::new("cat 0<input 2>>error && echo done > output")
            .lex()
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
        let tokens = Lexer::new("'2'>output")
            .lex()
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
        let tokens = Lexer::new("echo 2>&1 0<&3")
            .lex()
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
        let tokens = Lexer::new(r#"echo "\$x\q\\\"""#)
            .lex()
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
        assert_eq!(
            Lexer::new("echo \\").lex(),
            Err(LexerError::IncompleteEscape)
        );
        assert_eq!(
            Lexer::new("echo 'open").lex(),
            Err(LexerError::UnclosedSingleQuote)
        );
        assert_eq!(
            Lexer::new("echo \"open").lex(),
            Err(LexerError::UnclosedDoubleQuote)
        );
        assert_eq!(
            Lexer::new("a & b").lex(),
            Err(LexerError::UnsupportedOperator("&"))
        );
        assert_eq!(
            Lexer::new("cat << input").lex(),
            Err(LexerError::UnsupportedOperator("<<"))
        );
    }

    #[test]
    fn reports_an_io_number_that_does_not_fit_i32() {
        assert_eq!(
            Lexer::new("4294967296>output").lex(),
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
