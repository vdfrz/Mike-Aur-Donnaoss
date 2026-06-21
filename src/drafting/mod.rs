//! Drafting registry: per-case parties, annexures, and citations.
//!
//! Powers `@party` / `#annexure` cross-references (resolved at build time via
//! [`crossrefs`]), serial bookkeeping + AI seeding ([`registry`]), and the
//! auto-maintained "List of Cases Referred" / "List of Authorities"
//! ([`citations`]). Backed by migration `0038_drafting_registry.sql`.

pub mod citations;
pub mod crossrefs;
pub mod registry;
