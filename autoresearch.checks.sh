#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")/port"

# Run all tests — suppress success output, only show errors
cargo test --workspace 2>&1 | grep -E '(FAILED|error|panicked|test result)' || true
