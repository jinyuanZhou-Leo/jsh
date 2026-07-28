#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum Token {
    Word(Word),
    Redirect(RedirectOperator),
    IoNumber(u32),
    // &&
    AndAnd,
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
    /// 返回RedirectOperator默认左侧fd
    pub(crate) fn default_fd(self) -> u32 {
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
    pub(crate) fn from_parts(parts: Vec<WordPart>) -> Self {
        Self { parts }
    }

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
