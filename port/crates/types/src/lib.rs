#![allow(clippy::result_large_err)]

pub mod ast;
pub mod checker;
pub mod env;
pub mod errors;
pub mod exhaustiveness;
pub mod infer;
pub mod unify;

#[cfg(test)]
mod tests;
