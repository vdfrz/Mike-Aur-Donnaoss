//! Firm-knowledge corpus: ingest uploaded firm documents (past cases,
//! skeleton arguments, deeds, templates), chunk them structurally, tag
//! them with light metadata, and expose two agent tools the drafting
//! model uses to search the firm's own prior work instead of relying on
//! blind top-k injection.
//!
//! * `chunker` — pure structure-aware splitting (no DB, no LLM).
//! * `ingest`  — the pipeline: extract → chunk → store → LLM-tag.
//! * `tools`   — `search_firm_corpus` + `expand_chunk` exec fns.

pub mod chunker;
pub mod ingest;
pub mod tools;
