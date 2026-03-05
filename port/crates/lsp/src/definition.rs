use crate::hover::position_to_offset;
use marlang_core::span::Span;
use marlang_types::infer::InferContext;
use std::path::PathBuf;
use tower_lsp::lsp_types::Position;

/// The result of a definition lookup: a span and optionally a file path.
/// When `file_path` is `None`, the definition is in the same file.
/// When `Some`, it's a cross-file definition (e.g., from an imported module).
pub struct DefinitionLocation {
    pub span: Span,
    pub file_path: Option<PathBuf>,
}

/// Find all definition locations for the symbol at the given position.
///
/// Uses the InferContext's references, definitions, and cross_file_definitions to resolve:
/// 1. Find a reference at the cursor position (use_span contains the offset).
/// 2. Look up the name and SymbolKind from that reference.
/// 3. Collect same-file definitions matching both name and kind (most recent before the reference).
///    Definitions from the prelude (index < prelude_definition_count) are treated as cross-file
///    definitions pointing to `prelude.mar`, since their spans are byte offsets within that file.
/// 4. Collect all cross-file definitions matching both name and kind.
///
/// The SymbolKind matching ensures that, e.g., a variable `println` and an effect operation
/// `println` don't collide — go-to-definition on `(perform println ...)` only finds the
/// effect operation definition, not a variable with the same name.
///
/// Returns all matching definition locations. When the same name is imported from
/// multiple modules, all sources are returned so the editor can show all candidates.
pub fn find_definitions(
    context: &InferContext,
    source: &str,
    position: Position,
) -> Vec<DefinitionLocation> {
    let offset = position_to_offset(source, position);

    // Step 1: Find which reference the cursor is on
    let reference = match context
        .references
        .iter()
        .find(|(span, _, _)| offset >= span.start && offset < span.end)
    {
        Some(r) => r,
        None => return vec![],
    };

    let ref_name = &reference.1;
    let ref_kind = reference.2;
    let ref_span = reference.0;

    let mut results = Vec::new();

    // Step 2: Try same-file and prelude definitions.
    // We want the last definition that was recorded before (or at) the reference span,
    // matching both name and SymbolKind.
    // This handles scoping: lambda params and let bindings are recorded in program order,
    // and the most recent one shadows earlier ones.
    //
    // Definitions with index < prelude_definition_count have spans that are byte offsets
    // within prelude.mar (not the user's file). Their spans cannot be compared with
    // the reference span (which is in the user's file), so they always match by name
    // and kind alone. They are returned as cross-file definitions pointing to prelude_path.
    let mut best_def: Option<(Span, bool)> = None; // (span, is_prelude)
    for (def_idx, (name, def_span, kind)) in context.definitions.iter().enumerate() {
        if name != ref_name || *kind != ref_kind {
            continue;
        }
        let is_prelude = def_idx < context.prelude_definition_count;
        if is_prelude {
            // Prelude definitions are always eligible — their spans are in a different
            // file so we can't compare them with the user-file reference span.
            // Later user-file definitions with the same name will shadow this.
            best_def = Some((*def_span, true));
        } else if def_span.start <= ref_span.start {
            // Same-file definition: must appear before the reference in source order.
            best_def = Some((*def_span, false));
        }
    }

    // Include the definition unless it's the reference itself (same span = cursor is on the def)
    if let Some((def_span, is_prelude)) = best_def
        && (def_span.start != ref_span.start || def_span.end != ref_span.end)
    {
        if is_prelude {
            // Prelude definition: route to the prelude.mar file on disk
            if let Some(prelude_path) = &context.prelude_path {
                results.push(DefinitionLocation {
                    span: def_span,
                    file_path: Some(prelude_path.clone()),
                });
            }
        } else {
            // User-file definition: same file
            results.push(DefinitionLocation {
                span: def_span,
                file_path: None,
            });
        }
    }

    // Step 3: Collect all cross-file definitions for this name and kind.
    for (imported_name, _, file_path, span, kind) in &context.cross_file_definitions {
        if imported_name == ref_name && *kind == ref_kind {
            results.push(DefinitionLocation {
                span: *span,
                file_path: Some(file_path.clone()),
            });
        }
    }

    results
}
