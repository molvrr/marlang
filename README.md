# Marlang

A Lisp-like language written in Rust. Features include a bytecode VM, a Haskell-style static type system, algebraic data types, pattern matching, algebraic effects with delimited continuations, closures, tail call optimization, green threads with channels, an actor model with supervision, and a Foreign Function Interface (FFI) via libffi.

## Features

- **Bytecode VM**: Stack-based bytecode compiler and virtual machine
- **Type System**: Static type checking with local inference, parametric polymorphism, row-polymorphic effects, and OCaml-style operator split (Int/Float). Primitive types: `Int`, `Float`, `Bool`, `String`, `Char`, `Unit`
- **Algebraic Data Types**: Define sum types with constructors
- **Pattern Matching**: Destructure values with `match` expressions, exhaustiveness checking
- **Algebraic Effects**: Delimited continuations with `effect`, `perform`, and `handle`
- **Concurrency**: Green threads (fibers), CSP-style channels, actor model with mailboxes and supervision
- **Module System**: Import code with `std:` prefix for stdlib or relative paths
- **Standard Library**: Auto-loaded prelude with Option, List, Pair, Result types; stdlib utilities out of the box
- **Tail Call Optimization**: Efficient recursion without stack overflow
- **Macros**: Hygienic macro system with `defmacro`, `gensym`, and quasiquoting
- First-class functions with lexical closures
- FFI for calling native libraries (via libffi)
- FFI struct support for C struct interop
- Async IO (TCP networking)
- Interactive REPL

## Building

```bash
# Build the CLI (creates 'mar' binary)
cargo build --release -p marlang-cli

# Or run directly via cargo
cargo run -p marlang-cli -- --help
```

## Running

Run a Marlang file:

```bash
cargo run -p marlang-cli -- run <filename.mar>
```

Disassemble a file to view bytecode:

```bash
cargo run -p marlang-cli -- disasm <filename.mar>
```

Type check without running (prints last expression's type):

```bash
cargo run -p marlang-cli -- typecheck <filename.mar>
```

Start the interactive REPL:

```bash
cargo run -p marlang-cli -- repl
```

Run the test suite:

```bash
cargo test --workspace
```

See [REFERENCE.md](REFERENCE.md) for the full language reference.

## Architecture

```
marlang/
├── port/                 # Rust implementation (Cargo workspace)
│   └── crates/
│       ├── core/         # marlang-core: lexer, parser, string interning, spans
│       ├── types/        # marlang-types: HM type inference, exhaustiveness
│       ├── vm/           # marlang-vm: compiler, VM, modules, FFI, schedulers, macros, TCP
│       └── cli/          # marlang-cli: clap-based CLI (run, repl, typecheck, disasm)
├── stdlib/               # Standard library (.mar files)
│   ├── prelude.mar       # Auto-loaded: data types, Exception effect, macros, core functions
│   ├── option.mar        # Option type utilities
│   ├── result.mar        # Result type and utilities
│   ├── list.mar          # List utilities
│   ├── io.mar            # IO effect and operations
│   ├── string.mar        # String utilities (40+ functions including char ops)
│   ├── math.mar          # Math utilities (constants, number theory, angle conversion)
│   ├── concurrent.mar    # Concurrency wrappers
│   ├── actor.mar         # Actor model
│   └── net.mar           # TCP networking wrappers
├── examples/             # Example programs
├── bench/                # Benchmark .mar programs
├── web/                  # Web playground (needs WASM port — see web/TODO.md)
├── ts-archive/           # Archived TypeScript implementation (historical reference)
│   ├── src/              # Original TS source (core, vm, types, modules)
│   ├── spec/             # Original TS test suite
│   ├── lsp/              # Original TS LSP server
│   └── ...
└── specs/                # Language specification notes
```

### Crate Structure

- **`marlang-core`** -- Lexer, parser, string interning (`intern.rs`), spans. Tokenizes source into tokens, parses into S-expression AST.
- **`marlang-types`** -- Hindley-Milner type inference, row-polymorphic effect types, exhaustiveness checking for `match`, FFI struct type validation.
- **`marlang-vm`** -- Bytecode compiler, tagged value representation, stack-based VM with algebraic effect system (handler frames, continuations), module loader, FFI via libffi, single-threaded and multi-threaded fiber schedulers, async IO, hygienic macro expansion, TCP networking.
- **`marlang-cli`** -- Clap-based CLI with `run`, `repl`, `typecheck`, `disasm` subcommands.

## Archived TypeScript Implementation

The original TypeScript implementation (which ran on Bun) is preserved in `ts-archive/` for historical reference. It includes the tree-walking interpreter, bytecode VM, type system, LSP server, and test suite. The Rust port is the active implementation.
