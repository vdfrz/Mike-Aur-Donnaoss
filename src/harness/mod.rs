//! The self-rewriting drafting harness — "Mike listens".
//!
//! Ported from the lavern government-proposal feedback loop (TypeScript), kept
//! to the half that fits Mike: the lawyer's free-text feedback is triaged by an
//! LLM into durable drafting rules ("lessons"), which are merged into a per-user
//! store, deprecated on request, and injected into every future draft's system
//! prompt. Each change bumps a generation counter so the UI can show lineage.
//!
//! What we deliberately dropped from lavern: there are no scored proposal
//! sections to revise live, so the document-revision + evaluator-scoring loop
//! and the section `planOverrides` have no analogue here. The compounding-
//! lessons + generation half is the part that actually "improves the harness".
//!
//! Mike drafts Indian legal documents, so a rule must never instruct the writer
//! to insert a specific party, date, figure, or citation (those are matter-
//! specific and must not be invented) — enforced in the triage prompt and a
//! post-hoc validator, mirroring lavern's grounding discipline.

use anyhow::Result;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::llm::oneshot::{self, OneshotConfig};

/// New outcome weight for the effectiveness EWMA (old keeps 0.7), per lavern.
const EWMA_WEIGHT: f64 = 0.3;
const MAX_RULE_CHARS: usize = 220;
const MIN_RULE_CHARS: usize = 8;
/// A rule must never tell the writer to add a hard figure, year, or percentage.
const BANNED_IN_RULE: &str = r"\$\s?\d|₹\s?\d|\b(19|20)\d{2}\b|\b\d+\s?(%|percent|people|staff)\b";

// ── Types ────────────────────────────────────────────────────────────────────

/// A learned drafting rule, as stored and as surfaced to the UI.
#[derive(Clone, serde::Serialize)]
pub struct Lesson {
    pub id: String,
    pub rule: String,
    pub kind: String, // "do" | "dont"
    pub effectiveness: f64,
    pub occurrences: i64,
    pub deprecated: bool,
}

/// A rule the lawyer wants Mike to learn this turn.
pub struct NewLesson {
    pub rule: String,
    pub kind: String,
}

/// One free-form message sorted into intent + channels by the judgment model.
#[derive(Default)]
pub struct Triage {
    pub intent: String, // feedback | question | mixed
    pub reply: String,
    pub lessons: Vec<NewLesson>,
    /// Short descriptions of existing rules to roll back ("forget the rule about X").
    pub retract: Vec<String>,
    pub feature_requests: Vec<String>,
}

/// One visible harness edit — what the chat animates as the rules are rewritten.
#[derive(serde::Serialize)]
pub struct HarnessEdit {
    /// "lesson-compiled" (new rule, green) | "lesson-retired" (rolled back, struck).
    pub kind: String,
    pub text: String,
}

/// Result of a deterministic evolution pass over one turn's triage.
pub struct EvolveResult {
    pub generation: i64,
    pub edits: Vec<HarnessEdit>,
}

// ── Rule identity / validation ───────────────────────────────────────────────

/// Collapse any run of non-alphanumerics to one space and lowercase, so
/// "onsite-only" and "onsite only" dedupe to the same lesson id.
fn normalize_rule(rule: &str) -> String {
    let mut out = String::with_capacity(rule.len());
    let mut prev_space = true;
    for ch in rule.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

/// Stable id from the normalized rule; case/punctuation collapse to one id.
fn lesson_id(rule: &str) -> String {
    let digest = Sha256::digest(normalize_rule(rule).as_bytes());
    let hex = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!("lsn-{}", &hex[..12])
}

/// Drop rules that are empty, too long, or would fight the grounding discipline.
fn validate_rule(rule: &str) -> bool {
    let r = rule.trim();
    if r.len() < MIN_RULE_CHARS || r.len() > MAX_RULE_CHARS {
        return false;
    }
    let re = regex::Regex::new(BANNED_IN_RULE).expect("valid banned-rule regex");
    !re.is_match(r)
}

// ── Triage (one LLM call) ────────────────────────────────────────────────────

const TRIAGE_SYSTEM: &str = "You are the feedback intake assistant for Mike, an AI legal-drafting assistant used by an Indian advocate. The lawyer is telling you how Mike should draft going forward.\n\n\
Classify the latest message intent: \"question\" (they are asking something — answer in the reply, change nothing), \"feedback\" (telling you what to do or avoid), or \"mixed\".\n\n\
For feedback, produce durable, GENERALIZED drafting rules: one imperative sentence each, applicable to future drafts, with NO facts specific to one matter. NEVER write a rule that tells the writer to insert a specific party name, date, figure, amount, or citation — those are matter-specific and must never be invented. Prefer rules about tone, structure, what to always include, what to avoid, formatting, and citation style.\n\n\
Detect RETRACTIONS: when the lawyer says to roll back, stop doing, forget, or undo a previous rule, put a short description of each such rule in \"retract\". A retraction must target one of MIKE'S CURRENT LEARNED RULES if that list is shown — never a rule you are adding this turn. When the lawyer asks what Mike has learned, answer from that list.\n\n\
Separate FEATURE REQUESTS: requests for NEW app capabilities Mike does not have yet (e.g. \"add a PDF export\", \"let me edit inline\"). These are NOT drafting preferences and must not become rules.\n\n\
Use the conversation history to resolve references. The reply must be 1-3 sentences, plain and courteous, and must never invent facts. Output JSON only.";

/// Tolerant JSON extractor shared with the rest of the loop (fenced or bare).
fn extract_json(text: &str) -> Option<serde_json::Value> {
    let body = if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        let after = after.strip_prefix("json").unwrap_or(after);
        match after.find("```") {
            Some(end) => &after[..end],
            None => text,
        }
    } else {
        text
    };
    let first_obj = body.find('{');
    let first_arr = body.find('[');
    let (start, end_ch) = match (first_obj, first_arr) {
        (Some(o), Some(a)) if a < o => (a, ']'),
        (Some(o), _) => (o, '}'),
        (None, Some(a)) => (a, ']'),
        (None, None) => return None,
    };
    let end = body.rfind(end_ch)?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&body[start..=end]).ok()
}

/// Triage the lawyer's message. One LLM call; tolerant of malformed output.
pub async fn triage(
    config: &OneshotConfig,
    message: &str,
    history: &[(String, String)], // (role, text), oldest first
    current_rules: &[Lesson],
) -> Triage {
    let rules_block = if current_rules.is_empty() {
        String::new()
    } else {
        let lines = current_rules
            .iter()
            .map(|l| format!("- ({}) {}", l.kind, l.rule))
            .collect::<Vec<_>>()
            .join("\n");
        format!("MIKE'S CURRENT LEARNED RULES:\n{lines}\n\n")
    };
    let history_block = history
        .iter()
        .rev()
        .take(10)
        .rev()
        .map(|(role, text)| {
            let who = if role == "you" { "LAWYER" } else { "MIKE" };
            let t: String = text.chars().take(600).collect();
            format!("{who}: {t}")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let user = format!(
        "{rules_block}{history}\n\nLAWYER'S LATEST MESSAGE:\n\"\"\"{message}\"\"\"\n\n\
         Return JSON only:\n\
         {{\"intent\":\"feedback\"|\"question\"|\"mixed\",\
         \"reply\":\"<1-3 sentence reply>\",\
         \"lessons\":[{{\"rule\":\"<one imperative sentence>\",\"kind\":\"do\"|\"dont\"}}],\
         \"retract\":[\"<short description of a rule to roll back>\"],\
         \"featureRequests\":[\"<new app capability>\"]}}",
        history = if history_block.is_empty() {
            String::new()
        } else {
            format!("CONVERSATION SO FAR:\n{history_block}")
        },
        message = message,
    );

    let raw = match oneshot::complete(config, TRIAGE_SYSTEM, &user).await {
        Ok(t) => t,
        Err(_) => return offline_triage(),
    };
    let Some(parsed) = extract_json(&raw) else {
        return offline_triage();
    };

    let intent = parsed
        .get("intent")
        .and_then(|v| v.as_str())
        .filter(|s| *s == "question" || *s == "mixed")
        .unwrap_or("feedback")
        .to_string();
    let reply = parsed
        .get("reply")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Understood — I've noted that.")
        .to_string();

    let lessons = parsed
        .get("lessons")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    let rule = l.get("rule").and_then(|v| v.as_str()).unwrap_or("").trim();
                    if !validate_rule(rule) {
                        return None;
                    }
                    let kind = if l.get("kind").and_then(|v| v.as_str()) == Some("dont") {
                        "dont"
                    } else {
                        "do"
                    };
                    Some(NewLesson {
                        rule: rule.to_string(),
                        kind: kind.to_string(),
                    })
                })
                .take(4)
                .collect()
        })
        .unwrap_or_default();

    let retract = str_array(&parsed, "retract");
    let feature_requests = str_array(&parsed, "featureRequests");

    Triage {
        intent,
        reply,
        lessons,
        retract,
        feature_requests,
    }
}

/// No judgment provider, or the call/parse failed: acknowledge and learn
/// nothing rather than polluting the harness with a garbage rule.
fn offline_triage() -> Triage {
    Triage {
        intent: "feedback".to_string(),
        reply: "Understood — I've noted your feedback.".to_string(),
        lessons: Vec::new(),
        retract: Vec::new(),
        feature_requests: Vec::new(),
    }
}

fn str_array(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

// ── Evolution (deterministic) ────────────────────────────────────────────────

/// Apply one turn's triage to the lesson store: merge new rules, roll back
/// retracted ones, and bump the generation if anything changed. Returns the
/// visible diff so the chat can animate the harness rewrite.
pub async fn evolve(db: &SqlitePool, user_id: &str, triage: &Triage) -> Result<EvolveResult> {
    let mut edits: Vec<HarnessEdit> = Vec::new();

    // Rules that existed before this turn. Retractions may only target these:
    // "stop using all-caps headings" arrives as BOTH a new lesson and a
    // retraction of the old habit, and matching the retraction against the
    // just-inserted lesson would kill it in the same turn it was learned.
    let pre_existing: Vec<(String, String)> =
        sqlx::query_as("SELECT id, rule FROM harness_lessons WHERE user_id = ? AND deprecated = 0")
            .bind(user_id)
            .fetch_all(db)
            .await?;
    // Grows as this turn inserts, so paraphrase-dedupe also catches the model
    // emitting the same rule twice in one turn.
    let mut active_rules = pre_existing.clone();

    // Merge new lessons.
    for lesson in &triage.lessons {
        let id = lesson_id(&lesson.rule);
        let existing: Option<(f64, i64, i64)> = sqlx::query_as(
            "SELECT effectiveness, occurrences, deprecated FROM harness_lessons WHERE user_id = ? AND id = ?",
        )
        .bind(user_id)
        .bind(&id)
        .fetch_optional(db)
        .await?;

        match existing {
            Some((eff, occ, deprecated)) => {
                // Reinforced: nudge effectiveness toward 1.0, bump occurrences,
                // un-deprecate (a human re-raised it). Only a reactivation is a
                // visible edit — silent reinforcement otherwise.
                let new_eff = eff * (1.0 - EWMA_WEIGHT) + EWMA_WEIGHT;
                sqlx::query(
                    "UPDATE harness_lessons SET effectiveness = ?, occurrences = ?, deprecated = 0, \
                     deprecation_reason = NULL, last_seen_at = datetime('now') WHERE user_id = ? AND id = ?",
                )
                .bind(new_eff)
                .bind(occ + 1)
                .bind(user_id)
                .bind(&id)
                .execute(db)
                .await?;
                if deprecated != 0 {
                    edits.push(HarnessEdit {
                        kind: "lesson-compiled".to_string(),
                        text: lesson.rule.clone(),
                    });
                }
            }
            None => {
                // The model rarely repeats a rule verbatim, so an exact-hash
                // miss may still be an existing rule reworded ("clearly
                // states" vs "states"). Reinforce that rule instead of
                // letting paraphrases pile up.
                if let Some((sim_id, _)) = active_rules
                    .iter()
                    .find(|(_, r)| similar_rules(r, &lesson.rule))
                    .cloned()
                {
                    sqlx::query(
                        "UPDATE harness_lessons SET effectiveness = effectiveness * ? + ?, \
                         occurrences = occurrences + 1, last_seen_at = datetime('now') \
                         WHERE user_id = ? AND id = ?",
                    )
                    .bind(1.0 - EWMA_WEIGHT)
                    .bind(EWMA_WEIGHT)
                    .bind(user_id)
                    .bind(&sim_id)
                    .execute(db)
                    .await?;
                    continue;
                }
                sqlx::query(
                    "INSERT INTO harness_lessons (user_id, id, rule, kind, effectiveness, occurrences) \
                     VALUES (?, ?, ?, ?, 0.5, 1)",
                )
                .bind(user_id)
                .bind(&id)
                .bind(&lesson.rule)
                .bind(&lesson.kind)
                .execute(db)
                .await?;
                active_rules.push((id.clone(), lesson.rule.clone()));
                edits.push(HarnessEdit {
                    kind: "lesson-compiled".to_string(),
                    text: lesson.rule.clone(),
                });
            }
        }
    }

    // Roll back retracted rules: match each description against the lessons
    // that existed before this turn (never the ones just added above).
    if !triage.retract.is_empty() {
        let mut retired: std::collections::HashSet<String> = std::collections::HashSet::new();
        for description in &triage.retract {
            if let Some((id, rule)) = best_match(description, &pre_existing) {
                if !retired.insert(id.clone()) {
                    continue;
                }
                sqlx::query(
                    "UPDATE harness_lessons SET deprecated = 1, deprecation_reason = ? \
                     WHERE user_id = ? AND id = ?",
                )
                .bind("Rolled back by the lawyer.")
                .bind(user_id)
                .bind(&id)
                .execute(db)
                .await?;
                edits.push(HarnessEdit {
                    kind: "lesson-retired".to_string(),
                    text: rule,
                });
            }
        }
    }

    // Bump the generation only when the harness actually changed.
    let generation = current_generation(db, user_id).await;
    if !edits.is_empty() {
        let next = generation + 1;
        sqlx::query(
            "INSERT INTO harness_state (user_id, generation, updated_at) VALUES (?, ?, datetime('now')) \
             ON CONFLICT(user_id) DO UPDATE SET generation = excluded.generation, updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(next)
        .execute(db)
        .await?;
        return Ok(EvolveResult { generation: next, edits });
    }
    Ok(EvolveResult { generation, edits })
}

/// Word-set Jaccard ≥ 0.8 between normalized rules — catches the same rule
/// reworded, which the exact lesson_id hash misses.
fn similar_rules(a: &str, b: &str) -> bool {
    let na = normalize_rule(a);
    let nb = normalize_rule(b);
    let aw: std::collections::HashSet<&str> = na.split(' ').filter(|w| !w.is_empty()).collect();
    let bw: std::collections::HashSet<&str> = nb.split(' ').filter(|w| !w.is_empty()).collect();
    if aw.is_empty() || bw.is_empty() {
        return false;
    }
    let inter = aw.intersection(&bw).count();
    let union = aw.len() + bw.len() - inter;
    inter as f64 / union as f64 >= 0.8
}

/// Find the active lesson whose rule best overlaps a retract description.
/// Requires at least one shared significant (>3 char) word.
fn best_match(description: &str, active: &[(String, String)]) -> Option<(String, String)> {
    let want: Vec<String> = significant_words(description);
    if want.is_empty() {
        return None;
    }
    let mut best: Option<(usize, &(String, String))> = None;
    for cand in active {
        let have = significant_words(&cand.1);
        let shared = want.iter().filter(|w| have.contains(w)).count();
        if shared >= 1 && best.map_or(true, |(b, _)| shared > b) {
            best = Some((shared, cand));
        }
    }
    best.map(|(_, c)| (c.0.clone(), c.1.clone()))
}

fn significant_words(s: &str) -> Vec<String> {
    normalize_rule(s)
        .split(' ')
        .filter(|w| w.len() > 3)
        .map(String::from)
        .collect()
}

// ── Read helpers (store + prompt injection) ──────────────────────────────────

pub async fn current_generation(db: &SqlitePool, user_id: &str) -> i64 {
    sqlx::query_as::<_, (i64,)>("SELECT generation FROM harness_state WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .map(|(g,)| g)
        .unwrap_or(0)
}

/// Active lessons, ranked by effectiveness then reinforcement.
pub async fn active_lessons(db: &SqlitePool, user_id: &str, limit: i64) -> Vec<Lesson> {
    sqlx::query_as::<_, (String, String, String, f64, i64)>(
        "SELECT id, rule, kind, effectiveness, occurrences FROM harness_lessons \
         WHERE user_id = ? AND deprecated = 0 \
         ORDER BY effectiveness DESC, occurrences DESC LIMIT ?",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, rule, kind, effectiveness, occurrences)| Lesson {
        id,
        rule,
        kind,
        effectiveness,
        occurrences,
        deprecated: false,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::similar_rules;

    #[test]
    fn paraphrase_counts_as_same_rule() {
        // The real pair from live testing: one word dropped, same rule.
        assert!(similar_rules(
            "End every legal notice with a prayer paragraph that clearly states the relief demanded.",
            "End every legal notice with a prayer paragraph that states the relief demanded.",
        ));
        // Different substance sharing structure must stay distinct.
        assert!(!similar_rules(
            "Use title case for all section headings.",
            "Use bold for all section headings.",
        ));
    }
}

/// The prompt block injected into every draft. Empty string when nothing learned.
pub async fn active_lessons_prompt(db: &SqlitePool, user_id: &str) -> String {
    let lessons = active_lessons(db, user_id, 12).await;
    if lessons.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## How Mike should draft (learned from this lawyer's feedback)\nApply these rules to every draft:\n",
    );
    let mut used = out.len();
    for l in lessons {
        let line = format!("- {}\n", l.rule);
        if used + line.len() > 1600 {
            break;
        }
        used += line.len();
        out.push_str(&line);
    }
    out
}
