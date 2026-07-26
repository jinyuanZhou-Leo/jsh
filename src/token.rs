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
    Input,
    // 覆盖写入
    OutputTruncate,
    // 追加写入
    OutputAppend,
}

impl RedirectOperator {
    /// 返回RedirectOperator对应的默认fd
    pub(crate) fn default_fd(self) -> u32 {
        // 小enum, 实现了Copy特征，直接传值即可
        match self {
            Self::Input => 0,
            Self::OutputAppend | Self::OutputTruncate => 1,
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
