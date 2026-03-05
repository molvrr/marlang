use marlang_core::span::{LineMap, Span};
use marlang_types::ast::{Type, format_type};
use marlang_types::infer::InferContext;
use tower_lsp::lsp_types::Position;

/// Convert a 0-indexed LSP Position to a byte offset in the source text.
pub fn position_to_offset(source: &str, position: Position) -> u32 {
    let target_line = position.line as usize;
    let target_char = position.character as usize;
    let mut current_line = 0;
    let mut offset = 0;

    for (i, ch) in source.char_indices() {
        if current_line == target_line {
            let col = i - offset;
            if col >= target_char {
                return i as u32;
            }
        }
        if ch == '\n' {
            if current_line == target_line {
                // Past the end of the target line — clamp to line end
                return i as u32;
            }
            current_line += 1;
            offset = i + 1;
        }
    }
    // At or past end of file
    source.len() as u32
}

/// Check if a span contains a byte offset.
fn span_contains(span: Span, offset: u32) -> bool {
    offset >= span.start && offset < span.end
}

/// Look up the type of an expression at a given position using the collected InferContext.
///
/// Finds the smallest span in the type_map that contains the offset,
/// which gives us the most specific type for the hovered expression.
pub fn type_at_position(
    context: &InferContext,
    source: &str,
    position: Position,
) -> Option<String> {
    let offset = position_to_offset(source, position);

    // Find the smallest (most specific) span that contains the offset
    let mut best: Option<&(Span, Type)> = None;
    for entry in &context.type_map {
        let span = entry.0;
        if span_contains(span, offset) {
            match best {
                None => best = Some(entry),
                Some(prev) => {
                    let prev_size = prev.0.end - prev.0.start;
                    let curr_size = span.end - span.start;
                    if curr_size < prev_size {
                        best = Some(entry);
                    }
                }
            }
        }
    }

    best.map(|(_, ty)| format_type(ty))
}

/// Convert a Span to an LSP Range using a LineMap.
pub fn span_to_range(line_map: &LineMap, span: Span) -> tower_lsp::lsp_types::Range {
    let (start_line, start_col) = line_map.resolve(span.start);
    let (end_line, end_col) = line_map.resolve(span.end);
    tower_lsp::lsp_types::Range {
        start: Position {
            line: start_line.saturating_sub(1),
            character: start_col.saturating_sub(1),
        },
        end: Position {
            line: end_line.saturating_sub(1),
            character: end_col.saturating_sub(1),
        },
    }
}
