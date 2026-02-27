/// Compact source span — 8 bytes total.
/// Byte offsets into the source string; line/column computed on demand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const DUMMY: Span = Span { start: 0, end: 0 };

    pub fn new(start: u32, end: u32) -> Self {
        Span { start, end }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// Line map built once per source file.
/// Binary-searches newline offsets to resolve (line, column).
pub struct LineMap {
    newline_offsets: Vec<u32>,
}

impl LineMap {
    pub fn new(source: &str) -> Self {
        let newline_offsets = source
            .bytes()
            .enumerate()
            .filter(|(_, b)| *b == b'\n')
            .map(|(i, _)| i as u32)
            .collect();
        LineMap { newline_offsets }
    }

    /// Resolve a byte offset to (line, column), both 1-indexed.
    pub fn resolve(&self, offset: u32) -> (u32, u32) {
        let line = self.newline_offsets.partition_point(|&nl| nl < offset) as u32;
        let col = if line > 0 {
            offset - self.newline_offsets[line as usize - 1] - 1
        } else {
            offset
        };
        (line + 1, col + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line() {
        let lm = LineMap::new("hello");
        assert_eq!(lm.resolve(0), (1, 1));
        assert_eq!(lm.resolve(4), (1, 5));
    }

    #[test]
    fn multi_line() {
        let lm = LineMap::new("ab\ncd\nef");
        assert_eq!(lm.resolve(0), (1, 1)); // 'a'
        assert_eq!(lm.resolve(3), (2, 1)); // 'c'
        assert_eq!(lm.resolve(6), (3, 1)); // 'e'
        assert_eq!(lm.resolve(7), (3, 2)); // 'f'
    }

    #[test]
    fn merge_spans() {
        let a = Span::new(5, 10);
        let b = Span::new(3, 12);
        let merged = a.merge(b);
        assert_eq!(merged, Span::new(3, 12));
    }
}
