//! How much work deep search puts into a message.

use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Light,
    #[default]
    Standard,
    Thorough,
    Max,
}

impl Effort {
    pub const ALL: [Self; 4] = [Self::Light, Self::Standard, Self::Thorough, Self::Max];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Standard => "standard",
            Self::Thorough => "thorough",
            Self::Max => "max",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|effort| effort.as_str() == text.trim())
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Standard => "Standard",
            Self::Thorough => "Thorough",
            Self::Max => "Max",
        }
    }

    /// What the user is told about it.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Light => "A quick pass over a few sources. Checks that links work.",
            Self::Standard => {
                "Answers every part of the question. Checks links and the key claims."
            }
            Self::Thorough => {
                "Keeps researching until every part is backed. Checks links and every claim."
            }
            Self::Max => {
                "Also looks for contrary evidence. Checks links and every claim, one by one."
            }
        }
    }

    /// What the model is told about how far to go (`{{depth}}` in the instructions).
    pub fn guidance(self) -> &'static str {
        match self {
            Self::Light => {
                "How far to go: light. Break the request into at most three sub-questions and \
                 read a few good sources. Stop as soon as they are answered."
            }
            Self::Standard => {
                "How far to go: standard. Break the request into three to six sub-questions. \
                 Keep going until each of them is answered by a source you read. Confirm the key \
                 facts in a second, independent source where you can."
            }
            Self::Thorough => {
                "How far to go: thorough. Break the request into all sub-questions an expert would \
                 ask. Keep going until each of them is answered, and every key fact (numbers, \
                 dates, claims) is confirmed by two independent sources. When sources disagree, \
                 find a more authoritative or more recent one. Do not stop early: use the budget \
                 while new searches still turn up something new."
            }
            Self::Max => {
                "How far to go: as far as possible. Break the request into all sub-questions an \
                 expert would ask, from several perspectives. Keep going until each of them is \
                 answered and every key fact is confirmed by two independent sources, at least one \
                 of them primary. Then search for contrary evidence: criticism, failed \
                 replications, corrections, newer results, and who disagrees and why, and revise \
                 what you found. Do not stop early: use the budget while new searches still turn \
                 up something new."
            }
        }
    }
}

/// What deep search may do per message at one effort level, and how its report is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeepBudget {
    /// Further calls are refused and the model is told to answer with what it has.
    pub max_searches: u32,
    pub max_fetches: u32,
    /// Results per search when the model does not ask for a number.
    pub max_results: usize,
    /// How many claims of the report are checked against their sources (those with numbers
    /// first). Links are always checked.
    pub max_claims: usize,
    /// Claims checked per question to the model: fewer is slower and more careful.
    pub claims_per_check: usize,
    /// How often the report may go back to the model because of what the checks found.
    pub max_revisions: u32,
    /// The question is split into at most this many sub-questions, which must be answered.
    pub max_questions: u32,
    /// The least research per sub-question, so that simple questions stay short. With fewer
    /// searches or pages the model is sent back to its research.
    pub searches_per_question: u32,
    pub pages_per_question: u32,
    /// Different sites to read, if that many pages are asked for.
    pub min_sites: u32,
    /// How often the model is sent back to its research at most.
    pub max_rounds: u32,
}

impl Default for DeepBudget {
    fn default() -> Self {
        Self::of(Effort::default())
    }
}

impl DeepBudget {
    pub fn of(effort: Effort) -> Self {
        match effort {
            Effort::Light => Self {
                max_searches: 10,
                max_fetches: 10,
                max_results: 8,
                max_claims: 0,
                claims_per_check: 1,
                max_revisions: 1,
                max_questions: 3,
                searches_per_question: 1,
                pages_per_question: 1,
                min_sites: 2,
                max_rounds: 1,
            },
            Effort::Standard => Self {
                max_searches: 30,
                max_fetches: 30,
                max_results: 10,
                max_claims: 12,
                claims_per_check: 6,
                max_revisions: 1,
                max_questions: 5,
                searches_per_question: 1,
                pages_per_question: 2,
                min_sites: 4,
                max_rounds: 2,
            },
            Effort::Thorough => Self {
                max_searches: 80,
                max_fetches: 80,
                max_results: 15,
                max_claims: 200,
                claims_per_check: 4,
                max_revisions: 2,
                max_questions: 8,
                searches_per_question: 2,
                pages_per_question: 2,
                min_sites: 7,
                max_rounds: 4,
            },
            Effort::Max => Self {
                max_searches: 160,
                max_fetches: 160,
                max_results: 20,
                max_claims: 200,
                claims_per_check: 1,
                max_revisions: 3,
                max_questions: 12,
                searches_per_question: 3,
                pages_per_question: 3,
                min_sites: 10,
                max_rounds: 8,
            },
        }
    }
}

/// The budget of each effort level.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Deep {
    pub light: DeepBudget,
    pub standard: DeepBudget,
    pub thorough: DeepBudget,
    pub max: DeepBudget,
}

impl Default for Deep {
    fn default() -> Self {
        Self {
            light: DeepBudget::of(Effort::Light),
            standard: DeepBudget::of(Effort::Standard),
            thorough: DeepBudget::of(Effort::Thorough),
            max: DeepBudget::of(Effort::Max),
        }
    }
}

/// Edits may leave out levels and limits: those are the level's own defaults.
impl<'de> Deserialize<'de> for Deep {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Levels {
            light: DeepBudget,
            standard: DeepBudget,
            thorough: DeepBudget,
            max: DeepBudget,
        }
        let mut whole = serde_json::to_value(Self::default()).map_err(D::Error::custom)?;
        merge(&mut whole, Value::deserialize(deserializer)?);
        let levels: Levels = serde_json::from_value(whole).map_err(D::Error::custom)?;
        Ok(Self {
            light: levels.light,
            standard: levels.standard,
            thorough: levels.thorough,
            max: levels.max,
        })
    }
}

fn merge(whole: &mut Value, edits: Value) {
    match (whole, edits) {
        (Value::Object(whole), Value::Object(edits)) => {
            for (key, value) in edits {
                merge(whole.entry(key).or_insert(Value::Null), value);
            }
        }
        (whole, edits) => *whole = edits,
    }
}

impl Deep {
    pub fn budget_mut(&mut self, effort: Effort) -> &mut DeepBudget {
        match effort {
            Effort::Light => &mut self.light,
            Effort::Standard => &mut self.standard,
            Effort::Thorough => &mut self.thorough,
            Effort::Max => &mut self.max,
        }
    }

    pub fn budget(&self, effort: Effort) -> DeepBudget {
        match effort {
            Effort::Light => self.light,
            Effort::Standard => self.standard,
            Effort::Thorough => self.thorough,
            Effort::Max => self.max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_effort_is_more_of_everything() {
        for pair in Effort::ALL.windows(2) {
            let (less, more) = (DeepBudget::of(pair[0]), DeepBudget::of(pair[1]));
            assert!(less.max_searches < more.max_searches);
            assert!(less.max_fetches < more.max_fetches);
            assert!(less.max_results < more.max_results);
            assert!(less.max_claims <= more.max_claims);
            assert!(less.max_revisions <= more.max_revisions);
            assert!(less.max_questions < more.max_questions);
            assert!(less.searches_per_question <= more.searches_per_question);
            assert!(less.pages_per_question <= more.pages_per_question);
            assert!(less.min_sites < more.min_sites);
            assert!(less.max_rounds < more.max_rounds);
        }
        for effort in Effort::ALL {
            // What has to be done is well within what may be done.
            let budget = DeepBudget::of(effort);
            assert!(budget.max_questions * budget.searches_per_question * 2 <= budget.max_searches);
            assert!(budget.max_questions * budget.pages_per_question * 2 <= budget.max_fetches);
            assert_eq!(Effort::parse(effort.as_str()), Some(effort));
            assert!(!effort.guidance().contains("  "));
        }
        assert_eq!(Effort::parse("extreme"), None);
    }

    /// A level that leaves something out gets that from its own level's defaults.
    #[test]
    fn a_level_can_be_edited_in_part() {
        let deep: Deep = serde_json::from_str(r#"{"max": {"max_searches": 500}}"#).unwrap();
        assert_eq!(deep.max.max_searches, 500);
        assert_eq!(deep.light, DeepBudget::of(Effort::Light));
        assert_eq!(
            deep.max.max_fetches,
            DeepBudget::of(Effort::Max).max_fetches
        );
    }
}
