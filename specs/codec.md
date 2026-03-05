# std:codec — Bidirectional Parser/Printer Library

## Summary

A single `Codec a` value that can both parse (`String -> Result String a`) and print (`a -> Result String String`). One definition, two directions. Replaces `std:parse`.

## Core Types

```lisp
(data (Iso a b) (MkIso (-> a (Option b)) (-> b (Option a))))
(data (Codec a) (MkCodec (-> Unit a) (-> a Unit)))

(effect PrintState
  (emit      (-> String Unit))
  (emit-char (-> Char Unit)))
```

`ParseState` (peek, consume, get-input, set-input) is reused internally from the existing parse machinery.

## Design Decisions

- **Codec Unit for syntax**: `seq-left` and `seq-right` require the discarded side to be `Codec Unit`. This is type-safe and natural — discarded positions are always literals, whitespace, or delimiters.
- **Partial isomorphisms**: `Iso a b` is the key composition primitive. Codecs are transformed via `iso` rather than arbitrary `map`.
- **Canonical printing**: The printer always produces a canonical form (e.g., one space for whitespace, no trailing separators).
- **Effect-based**: Parsers perform `ParseState` effects, printers perform `PrintState` effects. Handlers interpret both.

## API

### Runners

| Function | Type | Description |
|----------|------|-------------|
| `parse` | `(Codec a) -> String -> (Result String a)` | Run parser half |
| `print` | `(Codec a) -> a -> (Result String String)` | Run printer half |
| `round-trip` | `(Codec a) -> String -> (Result String String)` | Parse then print |

### Primitives

| Function | Type |
|----------|------|
| `lit-char` | `Char -> (Codec Unit)` |
| `lit-string` | `String -> (Codec Unit)` |
| `satisfy` | `(Char -> Bool) -> String -> (Codec Char)` |
| `any-char` | `Codec Char` |
| `digit` | `Codec Char` |
| `letter` | `Codec Char` |
| `space` | `Codec Unit` |
| `spaces` | `Codec Unit` |
| `eof` | `Codec Unit` |

### Combinators

| Function | Type |
|----------|------|
| `iso` | `(Iso a b) -> (Codec a) -> (Codec b)` |
| `seq` | `(Codec a) -> (Codec b) -> (Codec (Pair a b))` |
| `seq-left` | `(Codec a) -> (Codec Unit) -> (Codec a)` |
| `seq-right` | `(Codec Unit) -> (Codec a) -> (Codec a)` |
| `alt` | `(Codec a) -> (Codec a) -> (Codec a)` |
| `many` | `(Codec a) -> (Codec (List a))` |
| `many1` | `(Codec a) -> (Codec (List a))` |
| `sep-by` | `(Codec Unit) -> (Codec a) -> (Codec (List a))` |
| `sep-by1` | `(Codec Unit) -> (Codec a) -> (Codec (List a))` |
| `between` | `(Codec Unit) -> (Codec Unit) -> (Codec a) -> (Codec a)` |
| `optional` | `(Codec a) -> (Codec (Option a))` |
| `label` | `String -> (Codec a) -> (Codec a)` |

### Isomorphisms

| Function | Type |
|----------|------|
| `iso-id` | `Iso a a` |
| `iso-compose` | `(Iso a b) -> (Iso b c) -> (Iso a c)` |
| `iso-inverse` | `(Iso a b) -> (Iso b a)` |
| `iso-chars` | `Iso (List Char) String` |
| `iso-int` | `Iso String Int` |
| `iso-float` | `Iso String Float` |
| `iso-cons` | `Iso (Pair a (List a)) (List a)` |
| `iso-nil` | `Iso Unit (List a)` |
| `iso-some` | `Iso a (Option a)` |
| `iso-none` | `Iso Unit (Option a)` |

### Convenience / Compat

| Function | Type |
|----------|------|
| `parser-of` | `(Codec a) -> (-> Unit a)` |
| `printer-of` | `(Codec a) -> (-> a Unit)` |
| `pure` | `a -> (Codec a)` |
| `fail` | `String -> (Codec a)` |
| `chainl1` | `(-> Unit (-> a a a)) -> (-> Unit a) -> a` |
| `chainr1` | `(-> Unit (-> a a a)) -> (-> Unit a) -> a` |

## Risks

1. **Print-side backtracking**: `alt` printer needs `try-print` (fresh handler scope). Deep nesting may be slow.
2. **`chainl1`/`chainr1` are parse-only**: No natural bidirectional form for operator chains.
