use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Requirement {
    Equals(String, String),
    NotEquals(String, String),
    In(String, BTreeSet<String>),
    NotIn(String, BTreeSet<String>),
    Exists(String),
    DoesNotExist(String),
}

impl Requirement {
    fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        match self {
            Self::Equals(key, value) => labels.get(key) == Some(value),
            Self::NotEquals(key, value) => labels.get(key) != Some(value),
            Self::In(key, values) => labels.get(key).is_some_and(|value| values.contains(value)),
            Self::NotIn(key, values) => {
                !labels.get(key).is_some_and(|value| values.contains(value))
            }
            Self::Exists(key) => labels.contains_key(key),
            Self::DoesNotExist(key) => !labels.contains_key(key),
        }
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let set = |values: &BTreeSet<String>| values.iter().cloned().collect::<Vec<_>>().join(",");
        match self {
            Self::Equals(key, value) => write!(f, "{key}={value}"),
            Self::NotEquals(key, value) => write!(f, "{key}!={value}"),
            Self::In(key, values) => write!(f, "{key} in ({})", set(values)),
            Self::NotIn(key, values) => write!(f, "{key} notin ({})", set(values)),
            Self::Exists(key) => write!(f, "{key}"),
            Self::DoesNotExist(key) => write!(f, "!{key}"),
        }
    }
}

/// A label selector, matched as Kubernetes matches it. Every requirement must hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    requirements: Vec<Requirement>,
}

impl Selector {
    /// Parses the selector syntax that `kubectl -l` takes, such as `app=api,tier in (web,db)`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let requirements = split_top_level(text)
            .into_iter()
            .map(|part| parse_requirement(part.trim()))
            .collect::<Result<Vec<_>, _>>()?;
        if requirements.is_empty() {
            return Err("Enter a label selector, such as app=api".to_owned());
        }
        Ok(Self { requirements })
    }

    /// The selector of a workload. An unknown operator is an error rather than a
    /// selector that matches too much.
    pub(crate) fn from_label_selector(selector: &LabelSelector) -> Result<Self, String> {
        let mut requirements: Vec<_> = selector
            .match_labels
            .iter()
            .flatten()
            .map(|(key, value)| Requirement::Equals(key.clone(), value.clone()))
            .collect();
        for expression in selector.match_expressions.iter().flatten() {
            let key = expression.key.clone();
            let values = || expression.values.iter().flatten().cloned().collect();
            requirements.push(match expression.operator.as_str() {
                "In" => Requirement::In(key, values()),
                "NotIn" => Requirement::NotIn(key, values()),
                "Exists" => Requirement::Exists(key),
                "DoesNotExist" => Requirement::DoesNotExist(key),
                operator => return Err(format!("Unknown selector operator \"{operator}\"")),
            });
        }
        if requirements.is_empty() {
            return Err("The selector is empty, so it would match every pod".to_owned());
        }
        Ok(Self { requirements })
    }

    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        self.requirements
            .iter()
            .all(|requirement| requirement.matches(labels))
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (ix, requirement) in self.requirements.iter().enumerate() {
            if ix > 0 {
                f.write_str(",")?;
            }
            write!(f, "{requirement}")?;
        }
        Ok(())
    }
}

/// Splits on the commas between requirements, keeping those inside a value set.
fn split_top_level(text: &str) -> Vec<&str> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (ix, char) in text.char_indices() {
        match char {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&text[start..ix]);
                start = ix + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn parse_requirement(part: &str) -> Result<Requirement, String> {
    let invalid = || format!("\"{part}\" is not a label requirement");
    if let Some(key) = part.strip_prefix('!') {
        return Ok(Requirement::DoesNotExist(
            key_of(key.trim()).ok_or_else(invalid)?,
        ));
    }
    if let Some((key, value)) = part.split_once("!=") {
        return Ok(Requirement::NotEquals(
            key_of(key.trim()).ok_or_else(invalid)?,
            value_of(value.trim()).ok_or_else(invalid)?,
        ));
    }
    if let Some((key, value)) = part.split_once('=') {
        let value = value.strip_prefix('=').unwrap_or(value);
        return Ok(Requirement::Equals(
            key_of(key.trim()).ok_or_else(invalid)?,
            value_of(value.trim()).ok_or_else(invalid)?,
        ));
    }
    let Some((key, rest)) = part.split_once(char::is_whitespace) else {
        return Ok(Requirement::Exists(key_of(part).ok_or_else(invalid)?));
    };
    let key = key_of(key).ok_or_else(invalid)?;
    let rest = rest.trim_start();
    let (negated, set) = if let Some(set) = rest.strip_prefix("notin") {
        (true, set)
    } else if let Some(set) = rest.strip_prefix("in") {
        (false, set)
    } else {
        return Err(invalid());
    };
    let values = set
        .trim()
        .strip_prefix('(')
        .and_then(|set| set.strip_suffix(')'))
        .ok_or_else(invalid)?
        .split(',')
        .map(|value| value_of(value.trim()).filter(|value| !value.is_empty()))
        .collect::<Option<BTreeSet<_>>>()
        .filter(|values| !values.is_empty())
        .ok_or_else(invalid)?;
    Ok(if negated {
        Requirement::NotIn(key, values)
    } else {
        Requirement::In(key, values)
    })
}

/// A label key, which may carry a DNS prefix such as `app.kubernetes.io/name`.
fn key_of(key: &str) -> Option<String> {
    let valid = !key.is_empty()
        && key
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '-' | '_' | '.' | '/'));
    valid.then(|| key.to_owned())
}

fn value_of(value: &str) -> Option<String> {
    let valid = value
        .chars()
        .all(|char| char.is_ascii_alphanumeric() || matches!(char, '-' | '_' | '.'));
    valid.then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelectorRequirement;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn matches(selector: &str, pairs: &[(&str, &str)]) -> bool {
        Selector::parse(selector).unwrap().matches(&labels(pairs))
    }

    #[test]
    fn equality_requires_the_value() {
        assert!(matches("app=api", &[("app", "api"), ("tier", "web")]));
        assert!(matches("app==api", &[("app", "api")]));
        assert!(!matches("app=api", &[("app", "web")]));
        assert!(!matches("app=api", &[]));
    }

    #[test]
    fn inequality_matches_a_missing_label() {
        assert!(matches("app!=api", &[("app", "web")]));
        assert!(matches("app!=api", &[]));
        assert!(!matches("app!=api", &[("app", "api")]));
    }

    #[test]
    fn sets_and_existence_match_as_kubernetes_does() {
        let pod = [("app", "api"), ("tier", "web")];
        assert!(matches("tier in (web, db)", &pod));
        assert!(!matches("tier notin (web,db)", &pod));
        assert!(matches("tier notin (db)", &[]));
        assert!(matches("app", &pod));
        assert!(matches("!canary", &pod));
        assert!(!matches("!app", &pod));
    }

    #[test]
    fn every_requirement_must_hold() {
        let pod = [("app", "api"), ("tier", "web")];
        assert!(matches("app=api,tier in (web,db),!canary", &pod));
        assert!(!matches("app=api,tier=db", &pod));
    }

    #[test]
    fn keys_may_carry_a_prefix() {
        assert!(matches(
            "app.kubernetes.io/name=checkout",
            &[("app.kubernetes.io/name", "checkout")]
        ));
    }

    #[test]
    fn rejects_text_that_is_not_a_selector() {
        for text in [
            "",
            "  ",
            "app=",
            "=api",
            "app in web",
            "app in ()",
            "two words",
            "a=b c",
        ] {
            let parsed = Selector::parse(text);
            let empty_value = text == "app=";
            assert_eq!(parsed.is_ok(), empty_value, "{text:?} parsed as {parsed:?}");
        }
    }

    #[test]
    fn prints_in_the_syntax_it_parses() {
        let text = "app=api,tier in (db,web),track!=canary,!legacy,team";
        let selector =
            Selector::parse("app = api, tier in (web,db), track!=canary, !legacy, team").unwrap();

        assert_eq!(selector.to_string(), text);
        assert_eq!(Selector::parse(text).unwrap(), selector);
    }

    #[test]
    fn reads_a_workload_selector() {
        let selector = LabelSelector {
            match_labels: Some(labels(&[("app", "api")])),
            match_expressions: Some(vec![LabelSelectorRequirement {
                key: "track".into(),
                operator: "NotIn".into(),
                values: Some(vec!["canary".into()]),
            }]),
        };

        let selector = Selector::from_label_selector(&selector).unwrap();

        assert_eq!(selector.to_string(), "app=api,track notin (canary)");
        assert!(selector.matches(&labels(&[("app", "api")])));
        assert!(!selector.matches(&labels(&[("app", "api"), ("track", "canary")])));
    }

    #[test]
    fn refuses_a_workload_selector_that_would_match_everything() {
        assert!(Selector::from_label_selector(&LabelSelector::default()).is_err());
    }

    #[test]
    fn refuses_an_unknown_operator() {
        let selector = LabelSelector {
            match_expressions: Some(vec![LabelSelectorRequirement {
                key: "app".into(),
                operator: "Gt".into(),
                values: None,
            }]),
            ..LabelSelector::default()
        };

        assert!(Selector::from_label_selector(&selector).is_err());
    }
}
