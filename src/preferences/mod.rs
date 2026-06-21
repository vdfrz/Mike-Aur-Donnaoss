use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;

pub const VALID_CATEGORIES: &[&str] = &[
    "drafting_style",
    "citation_style",
    "research_strategy",
    "tone_profile",
    "practice_specialization",
    "anti_patterns",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceContext {
    Drafting,
    Research,
    Citation,
    CasePrep,
    GeneralChat,
}

impl PreferenceContext {
    pub fn relevant_categories(&self) -> &'static [&'static str] {
        match self {
            Self::Drafting => &["drafting_style", "tone_profile", "anti_patterns"],
            Self::Research => &["research_strategy", "citation_style", "tone_profile", "anti_patterns"],
            Self::Citation => &["citation_style", "tone_profile"],
            Self::CasePrep => &["practice_specialization", "tone_profile", "anti_patterns"],
            Self::GeneralChat => &["tone_profile", "practice_specialization", "anti_patterns"],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CategoryContent {
    #[serde(default)]
    pub critical_rules: Vec<String>,
    #[serde(default)]
    pub success_metrics: Vec<String>,
    #[serde(default)]
    pub anti_patterns: Vec<String>,
}

impl CategoryContent {
    pub fn is_empty(&self) -> bool {
        self.critical_rules.is_empty()
            && self.success_metrics.is_empty()
            && self.anti_patterns.is_empty()
    }

    pub fn merge_over(&self, base: &CategoryContent) -> CategoryContent {
        CategoryContent {
            critical_rules: if self.critical_rules.is_empty() {
                base.critical_rules.clone()
            } else {
                self.critical_rules.clone()
            },
            success_metrics: if self.success_metrics.is_empty() {
                base.success_metrics.clone()
            } else {
                self.success_metrics.clone()
            },
            anti_patterns: if self.anti_patterns.is_empty() {
                base.anti_patterns.clone()
            } else {
                self.anti_patterns.clone()
            },
        }
    }
}

pub type EffectivePreferences = HashMap<String, CategoryContent>;

pub async fn load_effective_preferences(
    db: &SqlitePool,
    user_id: &str,
    case_id: Option<&str>,
    context: PreferenceContext,
) -> EffectivePreferences {
    let relevant = context.relevant_categories();
    let mut result = EffectivePreferences::new();

    let user_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT category, content_json FROM user_preference_categories WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mut user_prefs: HashMap<String, CategoryContent> = HashMap::new();
    for (cat, json_str) in &user_rows {
        if relevant.contains(&cat.as_str()) {
            if let Ok(content) = serde_json::from_str::<CategoryContent>(json_str) {
                if !content.is_empty() {
                    user_prefs.insert(cat.clone(), content);
                }
            }
        }
    }

    let case_prefs: HashMap<String, CategoryContent> = if let Some(cid) = case_id {
        let case_rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT category, content_json FROM case_preferences WHERE case_id = ?",
        )
        .bind(cid)
        .fetch_all(db)
        .await
        .unwrap_or_default();

        let mut map = HashMap::new();
        for (cat, json_str) in &case_rows {
            if relevant.contains(&cat.as_str()) {
                if let Ok(content) = serde_json::from_str::<CategoryContent>(json_str) {
                    if !content.is_empty() {
                        map.insert(cat.clone(), content);
                    }
                }
            }
        }
        map
    } else {
        HashMap::new()
    };

    for &cat in relevant {
        let cat_str = cat.to_string();
        let effective = match (user_prefs.get(&cat_str), case_prefs.get(&cat_str)) {
            (Some(base), Some(override_)) => Some(override_.merge_over(base)),
            (Some(base), None) => Some(base.clone()),
            (None, Some(override_)) => Some(override_.clone()),
            (None, None) => None,
        };
        if let Some(content) = effective {
            if !content.is_empty() {
                result.insert(cat_str, content);
            }
        }
    }

    result
}

fn category_display_name(category: &str) -> &str {
    match category {
        "drafting_style" => "Drafting Style",
        "citation_style" => "Citation Style",
        "research_strategy" => "Research Strategy",
        "tone_profile" => "Tone Profile",
        "practice_specialization" => "Practice Specialization",
        "anti_patterns" => "Anti-Patterns",
        _ => category,
    }
}

pub fn format_preferences_prompt(prefs: &EffectivePreferences) -> String {
    if prefs.is_empty() {
        return String::new();
    }

    let mut sections = Vec::new();
    for cat in VALID_CATEGORIES {
        let cat_str = cat.to_string();
        if let Some(content) = prefs.get(&cat_str) {
            if content.is_empty() {
                continue;
            }
            let mut s = format!("## How this lawyer works — {}\n", category_display_name(cat));
            if !content.critical_rules.is_empty() {
                s.push_str("### Critical Rules\n");
                for rule in &content.critical_rules {
                    s.push_str(&format!("- {rule}\n"));
                }
            }
            if !content.success_metrics.is_empty() {
                s.push_str("### Success Metrics\n");
                for metric in &content.success_metrics {
                    s.push_str(&format!("- {metric}\n"));
                }
            }
            if !content.anti_patterns.is_empty() {
                s.push_str("### Anti-patterns\n");
                for pattern in &content.anti_patterns {
                    s.push_str(&format!("- {pattern}\n"));
                }
            }
            sections.push(s);
        }
    }

    if sections.is_empty() {
        return String::new();
    }

    let mut out = sections.join("\n");
    out.push_str(
        "\nThese preferences override default behavior. If they conflict with a system rule \
         (e.g., legal accuracy, citation requirements), follow the system rule and note the conflict to the lawyer.",
    );
    out
}
