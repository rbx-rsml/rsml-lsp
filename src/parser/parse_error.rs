use std::{borrow::Cow, cmp::min};

use levenshtein::levenshtein;
use serde_json::Value;
use tower_lsp::lsp_types::{DiagnosticSeverity, Range};

use crate::{collection};

#[derive(Debug, Clone, PartialEq)]
pub enum ParseErrorMessage<'a> {
    Expected(&'a str),
    NotAllowed { name: &'a str, context: Option<&'a str> },
    Correction { closest: Option<&'a str>, range: Range }
}

impl<'a> ParseErrorMessage<'a> {
    pub fn correction<const N: usize>(name: Option<String>, range: Range, allow_list: &[&'static str; N]) -> Self {
        Self::Correction {
            closest:
                if let Some(name) = name { calc_closest(name, allow_list) }
                else { None },
            range
        }
    }
}

impl<'a> ToString for ParseErrorMessage<'a> {
    fn to_string(&self) -> String {
        match self {
            Self::Expected(str) => format!("Expected {str}."),
            Self::NotAllowed { name, context } => 
                if let Some(context) = context { format!("{name} are not allowed in {context}.") }
                else { format!("{name} are not allowed here.") },
            Self::Correction { closest, .. } => {
                closest
                    .map(|x| format!("Did you mean {x}?"))
                    .unwrap_or_default()
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum TypeErrorKind {
    UnknownDerive
}

impl TypeErrorKind {
    fn to_str(&self) -> &str {
        match self {
            Self::UnknownDerive => "Unknown Derive"
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError<'a> {
    // Errors.
    UnexpectedTokens { msg: Option<ParseErrorMessage<'a>> },
    MissingToken { msg: Option<ParseErrorMessage<'a>> },
    TypeError { kind: TypeErrorKind, msg: Option<&'a str> },

    // Warnings.
    RedundantTokens { msg: Option<Cow<'a, str>> },
}

impl<'a> ParseError<'a> {
    pub fn severity(&self) -> DiagnosticSeverity {
        match self {
            Self::UnexpectedTokens { .. } |
            Self::MissingToken { .. } |
            Self::TypeError { .. } => DiagnosticSeverity::ERROR,

            Self::RedundantTokens { .. } => DiagnosticSeverity::WARNING
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::UnexpectedTokens { msg } => match msg {
                Some(msg) => format!("Unexpected Token(s): {}", msg.to_string()),
                None => String::from("Unexpected Token(s)")
            },

            Self::MissingToken { msg } => match msg {
                Some(msg) => format!("Missing Token: {}", msg.to_string()),
                None => String::from("Missing Token")
            },

            Self::TypeError { kind, msg } => match msg {
                Some(msg) => format!("Type Error ({}): {}", kind.to_str(), msg.to_string()),
                None =>  format!("Type Error ({})", kind.to_str())
            }

            Self::RedundantTokens { msg } => match msg {
                Some(msg) => format!("Redundant Token(s): {}", msg),
                None => String::from("Redundant Token(s)")
            },
        }
    }

    pub fn data(&self) -> Option<Value> {
        match self {
            Self::UnexpectedTokens {
                msg: Some(ParseErrorMessage::Correction { closest, range })
            } | Self::MissingToken {
                msg: Some(ParseErrorMessage::Correction { closest, range })
            } => {
                let (range_start, range_end) = (range.start, range.end);

                closest.as_ref().map(|x| {
                    Value::Object(collection!{
                        "range_start".to_string() => Value::Object(collection!{
                            "line".to_string() => Value::Number((range_start.line).into()),
                            "char".to_string() => Value::Number((range_start.character).into()),
                        }),
                        "range_end".to_string() => Value::Object(collection!{
                            "line".to_string() => Value::Number((range_end.line).into()),
                            "char".to_string() => Value::Number((range_end.character).into()),
                        }),
                        "closest".to_string() => Value::String(x.to_string()),
                    })
                })
            },
            _ => None
        }
    }
}

impl<'a> ToString for ParseError<'a> {
    fn to_string(&self) -> String {
         match self {
            Self::UnexpectedTokens { .. } => "UNEXPECTED_TOKENS".into(),
            Self::MissingToken { .. } => "MISSING_TOKEN".into(),
            Self::TypeError { kind, .. } => format!("TYPE_ERROR[{}]", kind.to_str()),
            Self::RedundantTokens { .. } => "REDUNDANT_TOKENS".into(),
        }
    }
}

pub fn calc_closest<'a, const N: usize>(name: String, allow_list: &[&'static str; N]) -> Option<&'a str> {
    let name_len = name.len();

    allow_list
        .iter()
        .map(|x| (levenshtein(&name[0..min(name_len, x.len())], x), *x))
        .min_by_key(|x| x.0)
        .map(|x| x.1)
}