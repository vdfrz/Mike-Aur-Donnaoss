//! `.mikeprj` — portable project archive for sharing between MikeRust users.
//!
//! ## File format (v1)
//!
//! A `.mikeprj` is a single binary file with the following layout:
//!
//! ```text
//! ┌────────────────────────┐
//! │ magic   : "MIKEPRJ\0"  │  8 bytes
//! │ version : u8           │  1 byte  (currently 1)
//! │ flags   : u8           │  1 byte  (bit 0 = encrypted)
//! │ email_h : [u8; 32]     │  SHA-256(normalize(recipient_email))
//! │ salt    : [u8; 16]     │  Argon2id salt (random per file)
//! │ nonce   : [u8; 12]     │  AES-GCM nonce
//! │ payload : variable     │  AES-256-GCM(zip(project_tree))
//! └────────────────────────┘
//! ```
//!
//! The payload is a ZIP archive containing:
//!
//! ```text
//! manifest.json        # schema version, author, project metadata
//! project.json         # project record (name, cm_number, created_at, ...)
//! documents/<id>/meta.json
//! documents/<id>/content.bin
//! tabular_reviews/<id>.json   (configuration only, no cells)
//! workflows/<id>.json         (custom only, builtins re-resolved by id)
//! chats/<id>.json             (only when --include-chats)
//! ```
//!
//! ## Sharing model (v1 = "weak email pinning")
//!
//! The exporter types the recipient's email; the file is encrypted with a
//! key derived from that email. On import, the recipient's MikeRust
//! checks whether the email associated with their local account hashes
//! to the same value as the one in the file header. If so, the file is
//! decrypted; otherwise the import is refused with a clear message.
//!
//! Limitations (intentional, documented):
//!  - Anyone who knows the recipient's email can decrypt the file. This
//!    is "trust through possession of the file + knowledge of email" —
//!    NOT cryptographically strong sharing.
//!  - Designed for inter-colleague exchange where the threat model is
//!    "casual interception", not active attackers.
//!  - A future v2 will add real authenticated sharing once we have an
//!    out-of-band channel (email/SMS) to deliver a one-time PIN.

pub mod crypto;
pub mod io;
pub mod manifest;
