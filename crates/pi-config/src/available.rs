//! The models pi can use on this computer, from what `pi --list-models` prints: those in
//! models.json, and those of API keys and sign-ins.

use std::collections::{BTreeSet, HashMap};

use crate::ProviderInfo;

/// A model and what to call it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// `provider/id`.
    pub value: String,
    pub label: String,
}

/// Reads the provider and ID of each model from the table `pi --list-models` prints. `None` when
/// it printed something else.
pub fn parse_list(output: &str) -> Option<Vec<(String, String)>> {
    let mut lines = output.lines();
    // Extensions may print something first.
    let columns = loop {
        let Some(line) = lines.next() else {
            return output.contains("No models available").then(Vec::new);
        };
        let header: Vec<&str> = line.split_whitespace().collect();
        if header.starts_with(&["provider", "model"]) {
            break header.len();
        }
    };
    let models = lines
        .filter_map(|line| {
            let cells: Vec<&str> = line.split_whitespace().collect();
            // An ID may have spaces; the columns around it do not.
            let id = cells.get(1..(cells.len() + 2).checked_sub(columns)?)?;
            (!id.is_empty()).then(|| (cells[0].to_owned(), id.join(" ")))
        })
        .collect();
    Some(models)
}

/// The models to choose from: what pi `listed`, or, as long as that is not known, those in
/// models.json and those a session has (`provider`, `id`, `name`).
pub fn choices(
    listed: Option<&[(String, String)]>,
    file: &[ProviderInfo],
    session: &[(String, String, String)],
) -> Vec<Choice> {
    let known: Vec<(&str, &str, &str)> = file
        .iter()
        .flat_map(|provider| {
            provider.models.iter().map(|model| {
                let spec = &model.spec;
                (provider.id.as_str(), spec.id.as_str(), spec.name.as_str())
            })
        })
        .chain(
            session
                .iter()
                .map(|(provider, id, name)| (provider.as_str(), id.as_str(), name.as_str())),
        )
        .collect();
    let names: HashMap<(&str, &str), &str> = known
        .iter()
        .filter(|(_, id, name)| !name.is_empty() && name != id)
        .map(|(provider, id, name)| ((*provider, *id), *name))
        .collect();
    let models: BTreeSet<(&str, &str)> = match listed {
        Some(listed) => listed
            .iter()
            .map(|(provider, id)| (provider.as_str(), id.as_str()))
            .collect(),
        None => known
            .iter()
            .map(|(provider, id, _)| (*provider, *id))
            .collect(),
    };
    models
        .into_iter()
        .map(|(provider, id)| {
            let value = format!("{provider}/{id}");
            let label = match names.get(&(provider, id)) {
                Some(name) => format!("{name} ({provider})"),
                None => value.clone(),
            };
            Choice { value, label }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::ModelsFile;

    const LIST: &str = "\
provider    model             context  max-out  thinking  images
openrouter  qwen/qwen3:free   262.1K   32.8K    yes       no    
spark       inkling-2-mini    1.0M     65.5K    yes       yes   
spark       my own model      262.1K   32.8K    yes       yes   
";

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(provider, id)| ((*provider).to_owned(), (*id).to_owned()))
            .collect()
    }

    #[test]
    fn reads_what_pi_prints() {
        let models = pairs(&[
            ("openrouter", "qwen/qwen3:free"),
            ("spark", "inkling-2-mini"),
            ("spark", "my own model"),
        ]);
        assert_eq!(parse_list(LIST), Some(models.clone()));
        assert_eq!(
            parse_list(&format!("An extension says hi\n{LIST}\n")),
            Some(models)
        );
        assert_eq!(
            parse_list("No models available. Use /login to log into a provider.\n  /docs\n"),
            Some(Vec::new())
        );
        assert_eq!(parse_list("Unknown option: --list-models"), None);
        assert_eq!(parse_list(""), None);
    }

    /// Both models of one provider can be chosen without any session.
    #[test]
    fn choices_without_a_session() {
        let file = ModelsFile::parse(
            r#"{"providers":{"spark":{"baseUrl":"http://h/v1","api":"openai-completions",
            "models":[{"id":"qwen","name":"Qwen"},{"id":"inkling"}]}}}"#,
        )
        .unwrap()
        .providers();
        let choice = |value: &str, label: &str| Choice {
            value: value.into(),
            label: label.into(),
        };
        let both = [
            choice("spark/inkling", "spark/inkling"),
            choice("spark/qwen", "Qwen (spark)"),
        ];
        assert_eq!(choices(None, &file, &[]), both);
        let listed = pairs(&[("spark", "inkling"), ("spark", "qwen")]);
        assert_eq!(choices(Some(&listed), &file, &[]), both);
        // What pi can't use is not offered; what it knows besides the file is.
        let listed = pairs(&[("openai", "gpt")]);
        let session = [("openai".into(), "gpt".into(), "GPT".into())];
        assert_eq!(
            choices(Some(&listed), &file, &session),
            [choice("openai/gpt", "GPT (openai)")]
        );
    }
}
