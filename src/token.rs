#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum Token {
    Word(Word),
    Redirect(RedirectOperator),
    IoNumber(i32),
    // &&
    AndAnd,
    OrOr,
    Pipeline,
    Semicolon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedirectOperator {
    // < 输入重定向
    Input,
    // > 输出重定向
    OutputTruncate,
    // >> 追加输出
    OutputAppend,
    // 文件描述符复制或关闭
    DuplicateOutput,
    DuplicateInput,
}

impl RedirectOperator {
    /// 返回重定向操作符默认作用的文件描述符。
    ///
    /// # Returns
    ///
    /// 输入类操作符返回 0，输出类操作符返回 1。
    pub(crate) fn default_fd(self) -> i32 {
        // 小enum, 实现了Copy特征，直接传值即可
        match self {
            Self::Input | Self::DuplicateInput => 0,
            Self::OutputAppend | Self::OutputTruncate | Self::DuplicateOutput => 1,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Word {
    parts: Vec<WordPart>,
}

impl Word {
    /// 使用保持源码顺序的 WordPart 列表创建 Word。
    ///
    /// # Arguments
    ///
    /// * `parts` - Word 中未引用、引用和转义部分的有序列表。
    ///
    /// # Returns
    ///
    /// 持有给定组成部分的 [`Word`]。
    pub(crate) fn from_parts(parts: Vec<WordPart>) -> Self {
        Self { parts }
    }

    /// 消费 Word 并按源码顺序迭代其组成部分。
    ///
    /// # Returns
    ///
    /// 拥有每个 [`WordPart`] 所有权的迭代器。
    pub(crate) fn into_parts(self) -> impl Iterator<Item = WordPart> {
        self.parts.into_iter()
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum WordPart {
    SingleQuoted(String),
    DoubleQuoted(String),
    Unquoted(String),
    Escaped(char),
}
