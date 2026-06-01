//! Indian statute database routes.
//!
//! Read-only APIs for searching, browsing, and mapping Indian statutes
//! stored in the local SQLite database (migration 0037).
//!
//!   GET /statutes/search                       — FTS5 full-text search
//!   GET /statutes/acts                         — list all acts
//!   GET /statutes/acts/:short_name             — act + all sections
//!   GET /statutes/section/:statute/:section    — single section + mappings
//!   GET /statutes/map                          — old↔new section mapping

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/search", get(search_sections))
        .route("/acts", get(list_acts))
        .route("/acts/{short_name}", get(get_act))
        .route("/section/{statute}/{section}", get(get_section))
        .route("/map", get(map_section))
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct SearchHit {
    statute: String,
    section_number: String,
    title: String,
    snippet: String,
}

#[derive(Serialize)]
struct Statute {
    id: i64,
    short_name: String,
    full_title: String,
    year: i64,
    status: String,
    replaced_by: Option<String>,
    category: Option<String>,
    language: Option<String>,
}

#[derive(Serialize)]
struct Section {
    id: i64,
    statute_id: i64,
    section_number: String,
    title: String,
    body: String,
}

#[derive(Serialize)]
struct Mapping {
    id: i64,
    old_statute: String,
    old_section: String,
    new_statute: String,
    new_section: String,
    mapping_type: Option<String>,
    notes: Option<String>,
}

#[derive(Serialize)]
struct ActDetail {
    #[serde(flatten)]
    act: Statute,
    sections: Vec<Section>,
}

#[derive(Serialize)]
struct SectionDetail {
    #[serde(flatten)]
    section: Section,
    statute_name: String,
    mappings: Vec<Mapping>,
}

#[derive(Serialize)]
struct MappingResult {
    #[serde(flatten)]
    mapping: Mapping,
    target_section: Option<Section>,
}

// ---------------------------------------------------------------------------
// GET /statutes/search
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    limit: Option<i64>,
}

async fn search_sections(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<SearchHit>>, StatusCode> {
    let q = params.q.trim();
    if q.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let limit = params.limit.unwrap_or(20).min(100);

    let rows = sqlx::query(
        "SELECT ss.id, s.short_name, ss.section_number, ss.title, \
                snippet(statute_sections_fts, 2, '<mark>', '</mark>', '...', 32) as snippet \
         FROM statute_sections_fts fts \
         JOIN statute_sections ss ON ss.id = fts.rowid \
         JOIN statutes s ON s.id = ss.statute_id \
         WHERE statute_sections_fts MATCH ?1 \
         ORDER BY rank \
         LIMIT ?2",
    )
    .bind(q)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let hits: Vec<SearchHit> = rows
        .iter()
        .map(|r| SearchHit {
            statute: r.get("short_name"),
            section_number: r.get("section_number"),
            title: r.get("title"),
            snippet: r.get("snippet"),
        })
        .collect();

    Ok(Json(hits))
}

// ---------------------------------------------------------------------------
// GET /statutes/acts
// ---------------------------------------------------------------------------

async fn list_acts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Statute>>, StatusCode> {
    let rows: Vec<(i64, String, String, i64, String, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as(
            "SELECT id, short_name, full_title, year, status, replaced_by, category, language \
             FROM statutes \
             ORDER BY year DESC, short_name",
        )
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let acts: Vec<Statute> = rows
        .into_iter()
        .map(|(id, short_name, full_title, year, status, replaced_by, category, language)| {
            Statute { id, short_name, full_title, year, status, replaced_by, category, language }
        })
        .collect();

    Ok(Json(acts))
}

// ---------------------------------------------------------------------------
// GET /statutes/acts/:short_name
// ---------------------------------------------------------------------------

async fn get_act(
    State(state): State<Arc<AppState>>,
    Path(short_name): Path<String>,
) -> Result<Json<ActDetail>, StatusCode> {
    let act_row: (i64, String, String, i64, String, Option<String>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT id, short_name, full_title, year, status, replaced_by, category, language \
             FROM statutes \
             WHERE short_name = ?",
        )
        .bind(&short_name)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let act = Statute {
        id: act_row.0,
        short_name: act_row.1,
        full_title: act_row.2,
        year: act_row.3,
        status: act_row.4,
        replaced_by: act_row.5,
        category: act_row.6,
        language: act_row.7,
    };

    let section_rows: Vec<(i64, i64, String, String, String)> = sqlx::query_as(
        "SELECT id, statute_id, section_number, title, body \
         FROM statute_sections \
         WHERE statute_id = ? \
         ORDER BY CAST(section_number AS INTEGER), section_number",
    )
    .bind(act.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let sections: Vec<Section> = section_rows
        .into_iter()
        .map(|(id, statute_id, section_number, title, body)| {
            Section { id, statute_id, section_number, title, body }
        })
        .collect();

    Ok(Json(ActDetail { act, sections }))
}

// ---------------------------------------------------------------------------
// GET /statutes/section/:statute/:section
// ---------------------------------------------------------------------------

async fn get_section(
    State(state): State<Arc<AppState>>,
    Path((statute, section)): Path<(String, String)>,
) -> Result<Json<SectionDetail>, StatusCode> {
    let row: (i64, i64, String, String, String) = sqlx::query_as(
        "SELECT ss.id, ss.statute_id, ss.section_number, ss.title, ss.body \
         FROM statute_sections ss \
         JOIN statutes s ON s.id = ss.statute_id \
         WHERE s.short_name = ? AND ss.section_number = ?",
    )
    .bind(&statute)
    .bind(&section)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    let sec = Section {
        id: row.0,
        statute_id: row.1,
        section_number: row.2,
        title: row.3,
        body: row.4,
    };

    let mapping_rows: Vec<(i64, String, String, String, String, Option<String>, Option<String>)> =
        sqlx::query_as(
            "SELECT id, old_statute, old_section, new_statute, new_section, mapping_type, notes \
             FROM statute_mappings \
             WHERE (old_statute = ?1 AND old_section = ?2) \
                OR (new_statute = ?1 AND new_section = ?2)",
        )
        .bind(&statute)
        .bind(&section)
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mappings: Vec<Mapping> = mapping_rows
        .into_iter()
        .map(|(id, old_statute, old_section, new_statute, new_section, mapping_type, notes)| {
            Mapping { id, old_statute, old_section, new_statute, new_section, mapping_type, notes }
        })
        .collect();

    Ok(Json(SectionDetail {
        statute_name: statute,
        section: sec,
        mappings,
    }))
}

// ---------------------------------------------------------------------------
// GET /statutes/map
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct MapParams {
    statute: String,
    section: String,
    direction: Option<String>,
}

async fn map_section(
    State(state): State<Arc<AppState>>,
    Query(params): Query<MapParams>,
) -> Result<Json<Vec<MappingResult>>, StatusCode> {
    let old_to_new = params.direction.as_deref().unwrap_or("old_to_new") == "old_to_new";

    let (query, target_statute_col, target_section_col) = if old_to_new {
        (
            "SELECT id, old_statute, old_section, new_statute, new_section, mapping_type, notes \
             FROM statute_mappings \
             WHERE old_statute = ? AND old_section = ?",
            "new_statute",
            "new_section",
        )
    } else {
        (
            "SELECT id, old_statute, old_section, new_statute, new_section, mapping_type, notes \
             FROM statute_mappings \
             WHERE new_statute = ? AND new_section = ?",
            "old_statute",
            "old_section",
        )
    };

    let mapping_rows: Vec<(i64, String, String, String, String, Option<String>, Option<String>)> =
        sqlx::query_as(query)
            .bind(&params.statute)
            .bind(&params.section)
            .fetch_all(&state.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut results = Vec::with_capacity(mapping_rows.len());

    for (id, old_statute, old_section, new_statute, new_section, mapping_type, notes) in mapping_rows
    {
        let (target_stat, target_sec) = if old_to_new {
            (&new_statute, &new_section)
        } else {
            (&old_statute, &old_section)
        };

        let target_section: Option<(i64, i64, String, String, String)> = sqlx::query_as(
            "SELECT ss.id, ss.statute_id, ss.section_number, ss.title, ss.body \
             FROM statute_sections ss \
             JOIN statutes s ON s.id = ss.statute_id \
             WHERE s.short_name = ? AND ss.section_number = ?",
        )
        .bind(target_stat)
        .bind(target_sec)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let target = target_section.map(|(sid, statute_id, section_number, title, body)| {
            Section { id: sid, statute_id, section_number, title, body }
        });

        results.push(MappingResult {
            mapping: Mapping {
                id,
                old_statute,
                old_section,
                new_statute,
                new_section,
                mapping_type,
                notes,
            },
            target_section: target,
        });
    }

    Ok(Json(results))
}
