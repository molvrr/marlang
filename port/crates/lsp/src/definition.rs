use crate::hover::position_to_offset;
use marlang_core::span::Span;
use marlang_types::infer::InferContext;
use tower_lsp::lsp_types::Position;

/// Find the definition span for the symbol at the given position.
///
/// Uses the InferContext's references and definitions to resolve:
/// 1. Find a reference at the cursor position (use_span contains the offset).
/// 2. Look up the name from that reference.
/// 3. Find the most recent definition for that name that appears before the reference.
///
/// Returns the definition span if found.
pub fn find_definition(context: &InferContext, source: &str, position: Position) -> Option<Span> {
    let offset = position_to_offset(source, position);

    // Step 1: Find which reference the cursor is on
    let reference = context
        .references
        .iter()
        .find(|(span, _)| offset >= span.start && offset < span.end)?;

    let ref_name = &reference.1;
    let ref_span = reference.0;

    // Step 2: Find the best definition for this name.
    // We want the last definition that was recorded before (or at) the reference span.
    // This handles scoping: lambda params and let bindings are recorded in program order,
    // and the most recent one shadows earlier ones.
    let mut best_def: Option<Span> = None;
    for (name, def_span) in &context.definitions {
        if name == ref_name {
            // Accept definitions that were recorded before the reference point.
            // Since definitions are recorded in traversal order, the last one
            // before our reference is the correct scoped definition.
            // We use the definition's span start as the ordering key.
            if def_span.start <= ref_span.start {
                best_def = Some(*def_span);
            }
        }
    }

    // Don't return a definition if it's the same span as the reference
    // (i.e., the user is hovering over the definition itself)
    if let Some(def) = best_def
        && def.start == ref_span.start
        && def.end == ref_span.end
    {
        return None;
    }

    best_def
}
