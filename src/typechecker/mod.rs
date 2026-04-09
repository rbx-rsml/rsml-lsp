use std::{
    collections::HashMap,
    ops::{Deref, DerefMut, RangeInclusive},
    path::{Path, PathBuf},
};

use crate::{
    Document,
    luaurc::Luaurc,
    parser::{AstErrors, Construct, Parser},
};

use rangemap::RangeInclusiveMap;
use tower_lsp::lsp_types::{Diagnostic, NumberOrString, Range};

mod derive;
mod selectors;
mod type_error;

pub use type_error::*;

pub trait PushTypeError {
    fn push(&mut self, error: TypeError, range: Range);
}

impl PushTypeError for AstErrors {
    fn push(&mut self, error: TypeError, range: Range) {
        self.0.push(Diagnostic {
            range,
            severity: Some((&error).severity()),
            code: Some(NumberOrString::String((&error).to_string())),
            code_description: None,
            source: Some(String::from("RSML LSP")),
            message: error.message(),
            related_information: None,
            tags: None,
            data: error.data(),
        });
    }
}

pub struct Definitions(RangeInclusiveMap<usize, DefinitionKind>);

impl Definitions {
    pub fn new() -> Self {
        Self(RangeInclusiveMap::new())
    }
}

impl Deref for Definitions {
    type Target = RangeInclusiveMap<usize, DefinitionKind>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Definitions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(PartialEq, Eq, Clone)]
pub enum DefinitionKind {
    Derive {
        path: PathBuf,
    },
    Selector {
        type_definition: Vec<String>,
        hint: String,
    },
    Scope {
        type_definition: Vec<String>,
    },
    Assignment {
        property_name: String,
        type_definition: Vec<String>,
    },
    EnumName,
    EnumVariant {
        enum_name: String,
    },
}

impl DefinitionKind {
    fn selector_hint(classes: &Vec<String>) -> String {
        classes.join(" | ")
    }

    pub fn selector(type_definition: Vec<String>) -> Self {
        let hint = Self::selector_hint(&type_definition);
        Self::Selector {
            type_definition,
            hint,
        }
    }
}

pub struct Typechecker<'a> {
    pub parsed: Parser<'a>,
}

impl<'a> Typechecker<'a> {
    pub async fn new(
        parsed: Parser<'a>,
        current_path: &Path,
        mut luaurc: Option<&mut Luaurc>,
        document: &mut Document,
    ) -> (Self, HashMap<PathBuf, RangeInclusive<usize>>) {
        let mut typechecker: Typechecker<'a> = Self { parsed };

        // We need to use a different ast errors
        // vec due to borrow checker issues.
        let mut ast_errors = AstErrors::new();

        let mut derives: HashMap<PathBuf, RangeInclusive<usize>> = HashMap::new();

        for datatype in &typechecker.parsed.ast {
            match datatype {
                Construct::Derive {
                    body: Some(datatype),
                    ..
                } => {
                    typechecker
                        .typecheck_derive(
                            datatype,
                            &mut ast_errors,
                            current_path,
                            luaurc.as_deref_mut(),
                            document,
                            &mut derives,
                        )
                        .await;
                }

                Construct::Rule { selectors, body } => {
                    typechecker.typecheck_rule(
                        (selectors, body),
                        &vec![],
                        &mut ast_errors,
                        document,
                    );
                }

                _ => (),
            }
        }

        typechecker.parsed.ast_errors.0.extend(ast_errors.0);

        (typechecker, derives)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, lexer::Lexer, parser::Parser};
    use std::path::PathBuf;

    struct TypecheckResult {
        selectors: Vec<(usize, usize, Vec<String>)>,
        scopes: Vec<(usize, usize, Vec<String>)>,
        errors: Vec<String>,
    }

    async fn typecheck(source: &str) -> TypecheckResult {
        let lexer = Lexer::new(source);
        let parsed = Parser::new(lexer);
        let mut document = Document::new(source.to_string());
        let dummy_path = PathBuf::from("/test.rsml");

        let (typechecker, _derives) =
            Typechecker::new(parsed, &dummy_path, None, &mut document).await;

        let selectors: Vec<(usize, usize, Vec<String>)> = document
            .definitions
            .iter()
            .filter_map(|(range, kind)| {
                if let DefinitionKind::Selector {
                    type_definition, ..
                } = kind
                {
                    Some((*range.start(), *range.end(), type_definition.clone()))
                } else {
                    None
                }
            })
            .collect();

        let scopes: Vec<(usize, usize, Vec<String>)> = document
            .definitions
            .iter()
            .filter_map(|(range, kind)| {
                if let DefinitionKind::Scope {
                    type_definition, ..
                } = kind
                {
                    Some((*range.start(), *range.end(), type_definition.clone()))
                } else {
                    None
                }
            })
            .collect();

        let errors: Vec<String> = typechecker
            .parsed
            .ast_errors
            .0
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect();

        TypecheckResult {
            selectors,
            scopes,
            errors,
        }
    }

    // ── Top-level selectors ────────────────────────────────────────

    #[tokio::test]
    async fn simple_class_selector() {
        let result = typecheck("Frame {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn class_with_pseudo_selector() {
        let result = typecheck("Frame ::UIPadding {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["UIPadding"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn class_with_state_selector() {
        let result = typecheck("Frame :hover {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn comma_separated_selectors() {
        let result = typecheck("Frame, TextButton {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Frame", "TextButton"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn invalid_class_name() {
        let result = typecheck("NotARealClass {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Instance"]);
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("No class named \"NotARealClass\" exists"));
    }

    #[tokio::test]
    async fn invalid_pseudo_not_a_class() {
        let result = typecheck("Frame ::NotAClass {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Instance"]);
        assert!(
            result
                .errors
                .iter()
                .any(|err| err.contains("No class named \"NotAClass\" exists"))
        );
    }

    #[tokio::test]
    async fn invalid_pseudo_not_allowed() {
        let result = typecheck("Frame ::Frame {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert!(
            result
                .errors
                .iter()
                .any(|err| err.contains("can't be used as a Pseudo instance"))
        );
    }

    #[tokio::test]
    async fn invalid_state_selector() {
        let result = typecheck("Frame :notastate {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert!(
            result
                .errors
                .iter()
                .any(|err| err.contains("No state named \"notastate\" exists"))
        );
    }

    #[tokio::test]
    async fn nested_class_without_combinator_errors() {
        let result = typecheck("Frame { TextButton {} }").await;
        assert_eq!(result.selectors.len(), 2);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert_eq!(result.selectors[1].2, vec!["TextButton"]);
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("can't be nested"));
    }

    #[tokio::test]
    async fn nested_child_selector() {
        let result = typecheck("Frame { > TextButton {} }").await;
        assert_eq!(result.selectors.len(), 2);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert_eq!(result.selectors[1].2, vec!["TextButton"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn nested_pseudo_selector() {
        let result = typecheck("Frame { ::UIPadding {} }").await;
        assert_eq!(result.selectors.len(), 2);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert_eq!(result.selectors[1].2, vec!["UIPadding"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn nested_state_selector() {
        let result = typecheck("Frame { :hover {} }").await;
        assert_eq!(result.selectors.len(), 2);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert_eq!(result.selectors[1].2, vec!["Frame"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn multiple_nesting_levels() {
        let result = typecheck("Frame { TextButton { TextLabel {} } }").await;
        assert_eq!(result.selectors.len(), 3);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert_eq!(result.selectors[1].2, vec!["TextButton"]);
        assert_eq!(result.selectors[2].2, vec!["TextLabel"]);
        assert_eq!(result.errors.len(), 2);
        assert!(
            result
                .errors
                .iter()
                .all(|err| err.contains("can't be nested"))
        );
    }

    #[tokio::test]
    async fn nested_child_combinator_with_nesting() {
        let result = typecheck("Frame { > TextButton { > TextLabel {} } }").await;
        assert_eq!(result.selectors.len(), 3);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert_eq!(result.selectors[1].2, vec!["TextButton"]);
        assert_eq!(result.selectors[2].2, vec!["TextLabel"]);
        assert!(result.errors.is_empty());
    }

    // ── Top-level combinator class resolution ──────────────────────

    #[tokio::test]
    async fn top_level_child_selector_resolves_to_child() {
        let result = typecheck("Frame > TextButton {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["TextButton"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn top_level_child_with_pseudo_resolves_to_pseudo() {
        let result = typecheck("Frame > TextButton ::UIPadding {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["UIPadding"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn top_level_child_with_state_resolves_to_child() {
        let result = typecheck("Frame > TextButton :hover {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["TextButton"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn top_level_comma_still_resolves_both() {
        let result = typecheck("Frame, TextButton {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Frame", "TextButton"]);
        assert!(result.errors.is_empty());
    }

    // ── Name selector coerces to Instance ──────────────────────────

    #[tokio::test]
    async fn top_level_chain_with_name_selector_coerces_to_instance() {
        let result = typecheck("Frame > TextButton > .Hello {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Instance"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn top_level_child_with_name_selector_coerces_to_instance() {
        let result = typecheck("Frame > .Hello {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Instance"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn nested_child_with_name_selector_coerces_to_instance() {
        let result = typecheck("Frame { > .Hello {} }").await;
        assert_eq!(result.selectors.len(), 2);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert_eq!(result.selectors[1].2, vec!["Instance"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn chain_with_tag_then_comma() {
        let result = typecheck("Frame >> TextButton > .Hello, Frame {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Instance", "Frame"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn tag_selector_then_comma_at_top_level() {
        let result = typecheck(".Hello, TextButton {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Instance", "TextButton"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn nested_tag_then_comma() {
        let result = typecheck("Frame { > .Hello, > TextButton {} }").await;
        assert_eq!(result.selectors.len(), 2);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert_eq!(result.selectors[1].2, vec!["Instance", "TextButton"]);
        assert!(result.errors.is_empty());
    }

    // ── Deduplication ─────────────────────────────────────────────

    #[tokio::test]
    async fn duplicate_comma_selectors_are_deduplicated() {
        let result = typecheck("Frame, Frame, TextButton {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Frame", "TextButton"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn all_duplicate_selectors() {
        let result = typecheck("Frame, Frame, Frame {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn duplicate_with_combinator() {
        let result = typecheck("Frame > TextButton, Frame > TextButton {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["TextButton"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn duplicate_instance_coercion() {
        let result = typecheck(".Hello, .World {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Instance"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn duplicate_with_state_selectors() {
        let result = typecheck("Frame :hover, Frame :press {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn duplicate_pseudo_selectors() {
        let result = typecheck("Frame ::UIPadding, TextButton ::UIPadding {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["UIPadding"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn nested_duplicate_selectors() {
        let result = typecheck("Frame { > TextButton, > TextButton {} }").await;
        assert_eq!(result.selectors.len(), 2);
        assert_eq!(result.selectors[0].2, vec!["Frame"]);
        assert_eq!(result.selectors[1].2, vec!["TextButton"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn no_dedup_different_types() {
        let result = typecheck("Frame, TextButton {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["Frame", "TextButton"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn preserves_order_after_dedup() {
        let result = typecheck("TextButton, Frame, TextButton {}").await;
        assert_eq!(result.selectors.len(), 1);
        assert_eq!(result.selectors[0].2, vec!["TextButton", "Frame"]);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn scope_inserted_for_rule_body() {
        let result = typecheck("Frame {}").await;
        assert_eq!(result.scopes.len(), 1);
        assert_eq!(result.scopes[0].2, vec!["Frame"]);
    }

    #[tokio::test]
    async fn scope_has_union_types() {
        let result = typecheck("Frame, TextButton {}").await;
        assert_eq!(result.scopes.len(), 1);
        assert_eq!(result.scopes[0].2, vec!["Frame", "TextButton"]);
    }

    #[tokio::test]
    async fn nested_scopes_have_correct_types() {
        let result = typecheck("Frame { > TextButton {} }").await;
        // Outer scope gets split by inner scope insertion, so 3 entries:
        // two halves of the outer Frame scope + the inner TextButton scope
        assert!(result.scopes.len() >= 2);
        let scope_types: Vec<&Vec<String>> = result.scopes.iter().map(|s| &s.2).collect();
        assert!(scope_types.contains(&&vec!["Frame".to_string()]));
        assert!(scope_types.contains(&&vec!["TextButton".to_string()]));
    }

    #[tokio::test]
    async fn scope_with_combinator() {
        let result = typecheck("Frame > TextButton {}").await;
        assert_eq!(result.scopes.len(), 1);
        assert_eq!(result.scopes[0].2, vec!["TextButton"]);
    }

    #[tokio::test]
    async fn scope_with_pseudo_selector() {
        let result = typecheck("Frame ::UIPadding {}").await;
        assert_eq!(result.scopes.len(), 1);
        assert_eq!(result.scopes[0].2, vec!["UIPadding"]);
    }
}
