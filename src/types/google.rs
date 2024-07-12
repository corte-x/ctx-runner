use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Response {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Part {
    text(String),
    inlineData {
        mimeType: String,
        data: String,
    },
    functionCall {
        name: String,
        args: serde_json::Value,
    },
    functionResponse {
        name: String,
        response: Response,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Content {
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub role: String,
}

impl Content {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    pub fn user(msg: String) -> Self {
        Content {
            parts: vec![Part::text(msg)],
            role: "user".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub contents: Vec<Content>,
    pub tools: [Tool; 1],
    #[serde(skip_serializing_if = "Content::is_empty")]
    pub system_instruction: Content,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Tool {
    functionDeclarations(Vec<Function>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub content: Option<Content>,
    pub index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub candidates: [Candidate; 1],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, PartialOrd)]
pub struct Function {
    #[serde(rename(deserialize = "Description"))]
    pub description: String,
    #[serde(
        skip_serializing_if = "super::openapi::Schema::is_null",
        default = "super::openapi::Schema::default"
    )]
    pub parameters: super::openapi::Schema,

    #[serde(default = "String::new", rename(deserialize = "Name"))]
    pub name: String,
    #[serde(skip_serializing)]
    #[serde(
        rename(deserialize = "Exec"),
        default = "String::new",
        skip_serializing_if = "Content::is_empty"
    )]
    pub exec: String,
}
