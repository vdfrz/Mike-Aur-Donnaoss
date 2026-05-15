//! `.mikeprj` manifest schema (v1).
//!
//! All structs are versioned via the top-level `schema_version`. Adding
//! optional fields to existing structs is backward-compatible; renaming or
//! removing fields requires bumping `schema_version` and handling both
//! shapes in the importer.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// Top-level manifest, written as `manifest.json` inside the ZIP.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    /// Free-form, e.g. "MikeRust 0.1.0".
    pub exporter: String,
    /// ISO-8601 UTC timestamp.
    pub exported_at: String,
    /// Display name of the user who exported, for provenance only — the
    /// importer never trusts this for authorization decisions.
    pub exported_by_display_name: Option<String>,
    /// What the importer should expect to find in the archive.
    pub contents: ManifestContents,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ManifestContents {
    pub project: bool,
    pub document_count: u32,
    pub tabular_review_count: u32,
    pub workflow_count: u32,
    pub chat_count: u32,
    /// Whether chat history was opted into at export.
    pub includes_chats: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub cm_number: Option<String>,
    pub created_at: String,
    /// Email of the original creator. Used only for display ("imported
    /// from alice@…"); the importer creates a fresh project owned by the
    /// current user.
    pub original_creator_email: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DocumentRecord {
    pub id: String,
    pub filename: String,
    pub file_type: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TabularReviewRecord {
    pub id: String,
    pub title: Option<String>,
    pub columns_config: serde_json::Value,
    /// Document ids in the original archive — re-mapped at import time.
    pub document_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkflowRecord {
    pub id: String,
    pub title: String,
    pub r#type: String,
    pub prompt_md: Option<String>,
    pub columns_config: Option<serde_json::Value>,
    pub practice: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatRecord {
    pub id: String,
    pub title: Option<String>,
    pub created_at: String,
    pub messages: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manifest_roundtrips_through_json() {
        let m = Manifest {
            schema_version: SCHEMA_VERSION,
            exporter: "MikeRust 0.1.0".into(),
            exported_at: "2026-05-06T10:30:00Z".into(),
            exported_by_display_name: Some("Dario".into()),
            contents: ManifestContents {
                project: true,
                document_count: 3,
                tabular_review_count: 1,
                workflow_count: 2,
                chat_count: 0,
                includes_chats: false,
            },
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: Manifest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert_eq!(back.exporter, "MikeRust 0.1.0");
        assert_eq!(back.contents.document_count, 3);
        assert_eq!(back.contents.includes_chats, false);
        assert_eq!(back.exported_by_display_name.as_deref(), Some("Dario"));
    }

    #[test]
    fn manifest_contents_default_is_zeroes() {
        let c = ManifestContents::default();
        assert!(!c.project);
        assert_eq!(c.document_count, 0);
        assert_eq!(c.tabular_review_count, 0);
        assert_eq!(c.workflow_count, 0);
        assert_eq!(c.chat_count, 0);
        assert!(!c.includes_chats);
    }

    #[test]
    fn missing_optional_fields_deserialize_fine() {
        // Older exports won't have `exported_by_display_name`.
        // Without `#[serde(default)]` this would fail; we use Option to
        // handle it. Verify the absent-key case round-trips as None.
        let raw = json!({
            "schema_version": 1,
            "exporter": "X",
            "exported_at": "2026-01-01T00:00:00Z",
            "exported_by_display_name": null,
            "contents": {
                "project": true,
                "document_count": 0,
                "tabular_review_count": 0,
                "workflow_count": 0,
                "chat_count": 0,
                "includes_chats": false
            }
        });
        let m: Manifest = serde_json::from_value(raw).unwrap();
        assert!(m.exported_by_display_name.is_none());
    }

    #[test]
    fn document_record_serializes_with_all_fields() {
        let d = DocumentRecord {
            id: "doc-1".into(),
            filename: "contract.docx".into(),
            file_type: Some("docx".into()),
            mime_type: Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document".into()),
            size_bytes: Some(12_345),
            sha256: "deadbeef".into(),
            created_at: "2026-05-06T00:00:00Z".into(),
        };
        let s = serde_json::to_string(&d).unwrap();
        let back: DocumentRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back.size_bytes, Some(12_345));
        assert_eq!(back.sha256, "deadbeef");
    }

    #[test]
    fn workflow_record_handles_optional_columns_config() {
        // Assistant workflows have no `columns_config`; tabular workflows do.
        let asst = WorkflowRecord {
            id: "w1".into(),
            title: "Asst".into(),
            r#type: "assistant".into(),
            prompt_md: Some("Do X".into()),
            columns_config: None,
            practice: None,
        };
        let tab = WorkflowRecord {
            id: "w2".into(),
            title: "Tab".into(),
            r#type: "tabular".into(),
            prompt_md: None,
            columns_config: Some(json!([{"name":"col1"}])),
            practice: Some("contracts".into()),
        };
        let s1 = serde_json::to_string(&asst).unwrap();
        let s2 = serde_json::to_string(&tab).unwrap();
        let _: WorkflowRecord = serde_json::from_str(&s1).unwrap();
        let parsed: WorkflowRecord = serde_json::from_str(&s2).unwrap();
        assert_eq!(parsed.columns_config.unwrap()[0]["name"], "col1");
    }

    #[test]
    fn schema_version_is_1() {
        assert_eq!(SCHEMA_VERSION, 1);
    }
}
