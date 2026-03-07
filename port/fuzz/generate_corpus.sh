#!/usr/bin/env bash
# Generate seed corpus for the source-level fuzzer from existing tests,
# stdlib, and example files.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CORPUS_DIR="$SCRIPT_DIR/corpus/source"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

mkdir -p "$CORPUS_DIR"

echo "Generating seed corpus in $CORPUS_DIR..."

# Counter for generated files
COUNT=0

# --- Extract test cases from tests.rs ---
# Each test calls check_ok("...") or check_err("...") or check("...") with
# multi-line Marlang source strings. We extract these as individual corpus files.

TEST_FILE="$SCRIPT_DIR/../crates/types/src/tests.rs"
if [ -f "$TEST_FILE" ]; then
    # Extract source strings from check/check_ok/check_err calls
    # These are typically enclosed in r#"..."# or "..."
    python3 -c "
import re, sys, hashlib

with open('$TEST_FILE', 'r') as f:
    content = f.read()

# Match raw string literals: r#\"...\"#
# and regular string literals in check/check_ok/check_err calls
patterns = [
    r'check(?:_ok|_err)?\(\s*r#\"(.*?)\"#',
    r'check(?:_ok|_err)?\(\s*\"((?:[^\"\\\\]|\\\\.)*)\"',
]

corpus_dir = '$CORPUS_DIR'
count = 0

for pattern in patterns:
    for match in re.finditer(pattern, content, re.DOTALL):
        source = match.group(1)
        # Unescape regular strings
        if not pattern.startswith(r'check.*r#'):
            source = source.replace('\\\\n', '\n').replace('\\\\\"', '\"')
        h = hashlib.md5(source.encode()).hexdigest()[:12]
        filename = f'{corpus_dir}/test_{count:03d}_{h}'
        with open(filename, 'w') as out:
            out.write(source)
        count += 1

print(f'Extracted {count} test cases from tests.rs')
" 2>/dev/null || echo "Warning: python3 extraction failed, skipping test extraction"
fi

# --- Copy stdlib files ---
for f in "$PROJECT_ROOT"/stdlib/*.mar; do
    if [ -f "$f" ]; then
        base=$(basename "$f" .mar)
        cp "$f" "$CORPUS_DIR/stdlib_${base}"
        COUNT=$((COUNT + 1))
    fi
done
echo "Copied stdlib files: $COUNT"

# --- Copy example files ---
EXAMPLES_COUNT=0
for f in "$PROJECT_ROOT"/examples/*.mar; do
    if [ -f "$f" ]; then
        base=$(basename "$f" .mar)
        cp "$f" "$CORPUS_DIR/example_${base}"
        EXAMPLES_COUNT=$((EXAMPLES_COUNT + 1))
    fi
done
echo "Copied example files: $EXAMPLES_COUNT"

# --- Add hand-crafted edge cases ---
cat > "$CORPUS_DIR/edge_empty" << 'EOF'
EOF

cat > "$CORPUS_DIR/edge_single_atom" << 'EOF'
42
EOF

cat > "$CORPUS_DIR/edge_nested_lambda" << 'EOF'
(lambda (x) (lambda (y) (lambda (z) (+ x (+ y z)))))
EOF

cat > "$CORPUS_DIR/edge_deep_let" << 'EOF'
(let ((a 1))
  (let ((b (+ a 1)))
    (let ((c (+ b 1)))
      (let ((d (+ c 1)))
        (+ a (+ b (+ c d)))))))
EOF

cat > "$CORPUS_DIR/edge_self_app" << 'EOF'
(lambda (x) (x x))
EOF

cat > "$CORPUS_DIR/edge_polymorphic_let" << 'EOF'
(let ((id (lambda (x) x)))
  (begin
    (id 42)
    (id "hello")
    (id #t)))
EOF

cat > "$CORPUS_DIR/edge_data_match" << 'EOF'
(data (Tree a) (Leaf a) (Node (Tree a) (Tree a)))
(define (tree-sum : (-> (Tree Int) Int))
  (lambda (t)
    (match t
      ((Leaf n) n)
      ((Node l r) (+ (tree-sum l) (tree-sum r))))))
(tree-sum (Node (Leaf 1) (Node (Leaf 2) (Leaf 3))))
EOF

cat > "$CORPUS_DIR/edge_effect_handler" << 'EOF'
(effect State
  (get (-> Unit Int))
  (put (-> Int Unit)))
(handle
  (begin
    (perform put 10)
    (perform get ()))
  ((get k) -> (resume k 0))
  ((put v k) -> (resume k (begin))))
EOF

cat > "$CORPUS_DIR/edge_mutual_recursion" << 'EOF'
(define (even? : (-> Int Bool))
  (lambda (n)
    (if (= n 0)
      #t
      (odd? (- n 1)))))
(define (odd? : (-> Int Bool))
  (lambda (n)
    (if (= n 0)
      #f
      (even? (- n 1)))))
EOF

cat > "$CORPUS_DIR/edge_variadic" << 'EOF'
(+ 1 2 3 4 5)
(string-append "a" "b" "c" "d")
(list 1 2 3 4 5)
EOF

cat > "$CORPUS_DIR/edge_complex_types" << 'EOF'
(define (f : (-> (-> Int Int) (List Int) (List Int)))
  (lambda (g xs)
    (match xs
      (Nil Nil)
      ((Cons h t) (Cons (g h) (f g t))))))
EOF

cat > "$CORPUS_DIR/edge_effect_row_poly" << 'EOF'
(define (with-logging : (-> (-> Unit {IO | e} a) {IO | e} a))
  (lambda (thunk)
    (begin
      (perform println "start")
      (let ((result (thunk)))
        (begin
          (perform println "end")
          result)))))
EOF

echo ""
echo "Seed corpus generation complete."
echo "Total files in corpus: $(ls "$CORPUS_DIR" | wc -l | tr -d ' ')"
