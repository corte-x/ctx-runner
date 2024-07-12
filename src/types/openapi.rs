use std::{borrow::Cow, collections::BTreeMap};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SchemaObj {
    #[serde(
        skip_serializing_if = "str::is_empty",
        default = "Default::default",
        rename(deserialize = "Type")
    )]
    pub r#type: String,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        default = "Vec::new",
        rename(deserialize = "Enum")
    )]
    pub r#enum: Vec<String>,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        default = "Vec::new",
        rename(deserialize = "Required")
    )]
    pub required: Vec<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename(deserialize = "Nullable")
    )]
    pub nullable: Option<bool>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default = "BTreeMap::new")]
    pub properties: BTreeMap<String, Schema>,
    #[serde(
        skip_serializing_if = "str::is_empty",
        default = "Default::default",
        rename(deserialize = "Description")
    )]
    pub description: String,
    #[serde(
        skip_serializing_if = "str::is_empty",
        default = "Default::default",
        rename(deserialize = "Format")
    )]
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none", rename(deserialize = "Items"))]
    pub items: Option<Box<Schema>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
pub enum Schema {
    #[serde(rename = "$ref")]
    Ref(String),
    Object(SchemaObj),
    #[default]
    null,
}

impl Schema {
    pub fn is_null(&self) -> bool {
        matches!(self, Schema::null)
    }
}
