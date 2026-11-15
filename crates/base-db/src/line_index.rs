//! 字节偏移 ↔ 行列的转换。
//!
//! 内部一律用 UTF-8 字节偏移；LSP 客户端若只支持 UTF-16 列，由调用方借助
//! [`LineIndex::utf16_col`] / [`LineIndex::offset_utf16`] 在边界上转换。

use syntax::{TextRange, TextSize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// 从 0 开始
    pub line: u32,
    /// 行内的 UTF-8 字节偏移
    pub col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    /// 每一行的起始偏移；第一个元素恒为 0
    line_starts: Vec<TextSize>,
    len: TextSize,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut line_starts = vec![TextSize::from(0)];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(TextSize::from(i as u32 + 1));
            }
        }
        LineIndex {
            line_starts,
            len: TextSize::of(text),
        }
    }

    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    pub fn line_col(&self, offset: TextSize) -> LineCol {
        let offset = offset.min(self.len);
        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        LineCol {
            line: line as u32,
            col: u32::from(offset - self.line_starts[line]),
        }
    }

    pub fn offset(&self, line_col: LineCol) -> Option<TextSize> {
        let start = *self.line_starts.get(line_col.line as usize)?;
        let offset = start + TextSize::from(line_col.col);
        (offset <= self.len).then_some(offset)
    }

    pub fn line_start(&self, line: u32) -> Option<TextSize> {
        self.line_starts.get(line as usize).copied()
    }

    /// 某一行的范围（不含行尾的换行符之后的内容）。
    pub fn line_range(&self, line: u32) -> Option<TextRange> {
        let start = self.line_start(line)?;
        let end = self
            .line_starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(self.len);
        Some(TextRange::new(start, end))
    }

    /// 把字节列换算成 UTF-16 码元列。
    pub fn utf16_col(&self, text: &str, line_col: LineCol) -> u32 {
        let Some(start) = self.line_start(line_col.line) else {
            return line_col.col;
        };
        let start = usize::from(start);
        let end = (start + line_col.col as usize).min(text.len());
        match text.get(start..end) {
            Some(prefix) => prefix.encode_utf16().count() as u32,
            None => line_col.col,
        }
    }

    /// 由行号和 UTF-16 码元列求字节偏移。列超出行尾时钳到行尾。
    pub fn offset_utf16(&self, text: &str, line: u32, utf16_col: u32) -> Option<TextSize> {
        let range = self.line_range(line)?;
        let line_text = text.get(usize::from(range.start())..usize::from(range.end()))?;
        let mut units = 0u32;
        let mut bytes = 0usize;
        for c in line_text.chars() {
            if units >= utf16_col || c == '\n' || c == '\r' {
                break;
            }
            units += c.len_utf16() as u32;
            bytes += c.len_utf8();
        }
        Some(range.start() + TextSize::from(bytes as u32))
    }

    /// 由行号和 UTF-8 字节列求偏移，列超出行尾时钳到行尾，并对齐到字符边界。
    pub fn offset_utf8_clamped(&self, text: &str, line: u32, col: u32) -> Option<TextSize> {
        let range = self.line_range(line)?;
        let mut offset = (usize::from(range.start()) + col as usize).min(usize::from(range.end()));
        while offset > 0 && !text.is_char_boundary(offset) {
            offset -= 1;
        }
        Some(TextSize::from(offset as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_roundtrip() {
        let text = "ab\ncd\n\nxyz";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 4);
        for offset in 0..=text.len() as u32 {
            let lc = index.line_col(TextSize::from(offset));
            assert_eq!(index.offset(lc), Some(TextSize::from(offset)));
        }
        assert_eq!(
            index.line_col(TextSize::from(3)),
            LineCol { line: 1, col: 0 }
        );
        assert_eq!(
            index.line_col(TextSize::from(6)),
            LineCol { line: 2, col: 0 }
        );
    }

    #[test]
    fn utf16_conversion() {
        // "汉" 占 3 个 UTF-8 字节、1 个 UTF-16 码元；"😀" 占 4 个字节、2 个码元
        let text = "a汉😀b\nx";
        let index = LineIndex::new(text);
        let b_offset = TextSize::from(text.find('b').unwrap() as u32);
        let lc = index.line_col(b_offset);
        assert_eq!(lc, LineCol { line: 0, col: 8 });
        assert_eq!(index.utf16_col(text, lc), 4);
        assert_eq!(index.offset_utf16(text, 0, 4), Some(b_offset));
        assert_eq!(index.offset_utf16(text, 0, 100), Some(TextSize::from(9)));
        assert_eq!(index.offset_utf16(text, 1, 1), Some(TextSize::from(11)));
    }
}
