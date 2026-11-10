use glob::Pattern;
use std::collections::HashMap;
use std::fmt;

/// The only variables a rule may reference.
pub const VARIABLES: &[&str] = &[
    "TEMP",
    "LOCALAPPDATA",
    "APPDATA",
    "USERPROFILE",
    "WINDIR",
    "PROGRAMDATA",
];

#[derive(Debug, PartialEq, Eq)]
pub enum ExpandError {
    Malformed,
    Unknown(String),
    Unset(String),
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => write!(f, "accolade non fermée"),
            Self::Unknown(name) => write!(f, "variable inconnue {{{name}}}"),
            Self::Unset(name) => write!(f, "variable non définie {{{name}}}"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Env(HashMap<String, String>);

impl Env {
    pub fn from_process() -> Self {
        Self::from_pairs(VARIABLES.iter().filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_string(), value))
        }))
    }

    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(pairs.into_iter().collect())
    }

    /// Replaces `{VAR}` with its value, escaped so the value is never read as a glob.
    pub fn expand(&self, template: &str) -> Result<String, ExpandError> {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find('{') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let end = after.find('}').ok_or(ExpandError::Malformed)?;
            let name = &after[..end];
            if !VARIABLES.contains(&name) {
                return Err(ExpandError::Unknown(name.to_string()));
            }
            let value = self
                .0
                .get(name)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ExpandError::Unset(name.to_string()))?;
            out.push_str(&Pattern::escape(value));
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }
}

/// Checks a path template without needing the variables to be set.
pub fn validate_template(template: &str) -> Result<(), String> {
    if !template.starts_with('{') {
        return Err("le chemin doit commencer par une {VARIABLE}".to_string());
    }
    if template.split(['/', '\\']).any(|part| part == "..") {
        return Err("« .. » est interdit dans un chemin".to_string());
    }
    let placeholders = Env::from_pairs(
        VARIABLES
            .iter()
            .map(|name| (name.to_string(), "x".to_string())),
    );
    let expanded = placeholders
        .expand(template)
        .map_err(|error| error.to_string())?;
    Pattern::new(&expanded).map_err(|error| error.to_string())?;
    Ok(())
}
