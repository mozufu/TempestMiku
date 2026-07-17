use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectSignature {
    pub name: String,
    pub args: ValueType,
    pub result: ValueType,
    pub errors: BTreeSet<String>,
    pub approval: String,
    pub resumable: bool,
    pub sensitive: bool,
}

impl EffectSignature {
    pub fn new(name: impl Into<String>, args: ValueType, result: ValueType) -> Self {
        Self {
            name: name.into(),
            args,
            result,
            errors: BTreeSet::new(),
            approval: "none".into(),
            resumable: false,
            sensitive: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueType {
    Any,
    Never,
    Null,
    Bool,
    Int,
    Decimal,
    String,
    Uri,
    List(Box<ValueType>),
    Record(BTreeMap<String, ValueType>),
    Function(Box<ValueType>, Box<ValueType>),
    Local(String),
}

#[derive(Debug, Clone, Default)]
pub struct CapabilityCatalog {
    effects: BTreeMap<String, EffectSignature>,
    schemes: BTreeSet<String>,
    grants: BTreeSet<String>,
}

impl CapabilityCatalog {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(mut self, signature: EffectSignature) -> Self {
        self.effects.insert(signature.name.clone(), signature);
        self
    }
    pub fn allow(mut self, name: impl Into<String>) -> Self {
        self.grants.insert(name.into());
        self
    }
    pub fn scheme(mut self, scheme: impl Into<String>) -> Self {
        self.schemes.insert(scheme.into());
        self
    }
    pub fn effect(&self, name: &str) -> Option<&EffectSignature> {
        self.effects.get(name)
    }
    pub fn permits(&self, name: &str) -> bool {
        self.grants.iter().any(|grant| {
            grant == name
                || grant
                    .strip_suffix(".*")
                    .is_some_and(|prefix| name == prefix || name.starts_with(&format!("{prefix}.")))
        })
    }
    pub fn has_scheme(&self, scheme: &str) -> bool {
        self.schemes.contains(scheme)
    }
}
