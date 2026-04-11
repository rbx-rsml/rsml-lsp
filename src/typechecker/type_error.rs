use std::path::PathBuf;

use tower_lsp::lsp_types::{DiagnosticSeverity};
use crate::normalize_path::NormalizePath;

pub enum Datatype {
    String,
    Number,
    Tween
}

impl ToString for Datatype {
    fn to_string(&self) -> String {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Tween => "number or (number, EasingStyle?, EasingDirection?)"
        }.into()
    }
}

pub enum CyclicKind<'a> {
    Internal,
    External(&'a str)
}

pub enum TypeError<'a> {
    UnknownDerive { path: Option<&'a str> },
    CyclicDerive { kind: CyclicKind<'a> },
    InvalidType { expected: Option<Datatype> },
    InvalidTweenArg { expected: &'a str },
    InvalidSelector { msg: Option<&'a str> }
}

impl<'a> TypeError<'a> {
    pub fn severity(&self) -> DiagnosticSeverity {
        match self {
            Self::UnknownDerive { .. } |
            Self::CyclicDerive { .. } |
            Self::InvalidType { .. } |
            Self::InvalidTweenArg { .. } |
            Self::InvalidSelector { .. } => DiagnosticSeverity::ERROR
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::UnknownDerive { path } => match path {
                Some(path) => format!(
                    "Type Error (Unknown Derive): {:#?}",
                    std::path::absolute(path)
                        .unwrap_or(PathBuf::from(path))
                        .normalize()
                ),
                None => String::from("Type Error (Unknown Derive)")
            },

            Self::CyclicDerive { kind } => match kind {
                    CyclicKind::Internal => String::from("Type Error (Cyclic Derive): Cannot derive the current Style Sheet."),
                    CyclicKind::External(ancestry_chain) => format!(
                        "Type Error (Cyclic Derive): {}",
                        ancestry_chain
                    ),
                },

            Self::InvalidType { expected } => match expected {
                Some(expected) => format!("Type Error (Invalid Type): Expected type `{}`.", expected.to_string()),
                None => String::from("Type Error (Invalid Type)")
            },

            Self::InvalidTweenArg { expected } =>
                format!("Type Error (Invalid Tween Argument): Expected `{}`.", expected),

            Self::InvalidSelector { msg } => match msg {
                Some(msg) => format!("Type Error (Invalid Selector): {}", msg),
                None => String::from("Type Error (Invalid Selector)")
            },
        }
    }

    pub fn data(&self) -> Option<serde_json::Value> {
        None
    }
}

impl<'a> ToString for TypeError<'a> {
    fn to_string(&self) -> String {
        format!("TYPE_ERROR({})", match self {
            Self::UnknownDerive { .. } => "UNKNOWN_DERIVE",
            Self::CyclicDerive { .. } => "CYCLIC_DERIVE",
            Self::InvalidType { .. } => "INVALID_TYPE",
            Self::InvalidTweenArg { .. } => "INVALID_TWEEN_ARG",
            Self::InvalidSelector { .. } => "INVALID_SELECTOR"
        })
    }
}