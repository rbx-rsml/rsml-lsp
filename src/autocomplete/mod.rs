use std::path::Path;

use rbx_rsml::parser::{Construct, ParsedRsml};
use rbx_rsml::typechecker::{DefinitionKind, Definitions};
use rbx_rsml::typechecker::luaurc::Luaurc;

mod normalize_path;
pub mod selectors;
pub mod tokens;
pub mod tween;
pub mod derive;
pub mod values;

pub fn build_definitions(
    parsed: &ParsedRsml<'_>,
    definitions: &mut Definitions,
    current_path: &Path,
    luaurc: Option<&Luaurc>,
) {
    for construct in &parsed.ast {
        match construct {
            Construct::Rule { selectors: _, body } => {
                selectors::build_rule_body_definitions(body, definitions);
            }

            Construct::Derive {
                body: Some(derive_body),
                ..
            } => {
                derive::build_derive_definitions(
                    derive_body,
                    current_path,
                    luaurc,
                    definitions,
                );
            }

            Construct::Tween {
                body: Some(body), ..
            } => {
                let span = construct.span();
                definitions.insert(span.0..=span.1, DefinitionKind::Declaration);
                tween::build_tween_definitions(body, definitions);
            }

            Construct::Assignment {
                right: Some(right), ..
            } => {
                tokens::walk_construct(right, definitions);
            }

            _ => (),
        }
    }
}
