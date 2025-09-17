use std::{collections::{BTreeMap, HashMap, HashSet, VecDeque}, mem::discriminant, ops::{Deref, DerefMut, RangeInclusive}, path::{Path, PathBuf}, pin::Pin, slice::Iter, sync::Arc};

use crate::{Document, guarded_unwrap, lexer::{MultilineString, SpannedToken, Token, TokenKind}, list::TokenKindList, luaurc::Luaurc, node_token_matches, normalize_path::NormalizePath, parser::{AstErrors, Construct, Delimited, Node, Parser}, range_from_span::RangeFromSpan, token_kind_list, workspaces::Documents};

mod type_error;
use phf_macros::phf_set;
use rangemap::{RangeInclusiveMap};
use ropey::Rope;
use tokio::sync::{Mutex, MutexGuard};
use tower_lsp::lsp_types::{Diagnostic, NumberOrString, Range};

pub use type_error::*;


type SelectorTypeDefinition = Vec<Vec<String>>;

struct SelectorMetadata {
    type_definition: Option<SelectorTypeDefinition>,
    has_pseudo_selectors: bool,
    class_count: usize
}

impl SelectorMetadata {
    fn empty() -> Self {
        Self {
            type_definition: None,
            has_pseudo_selectors: false,
            class_count: 0
        }
    }
}

struct SelectorMetadataRef<'a> {
    type_definition: Option<&'a SelectorTypeDefinition>,
    has_pseudo_selectors: bool,
    class_count: usize
}

impl SelectorMetadata {
    fn as_ref(&'_ self) -> SelectorMetadataRef<'_> {
        SelectorMetadataRef {
            type_definition: self.type_definition.as_ref(),
            has_pseudo_selectors: self.has_pseudo_selectors,
            class_count: self.class_count
        }
    }
}

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
    Derive { path: PathBuf },
    Selector {
        type_definition: Vec<String>,
        hint: String
    }
}

impl DefinitionKind {
    fn selector_hint(classes: &Vec<String>) -> String {
        classes.join(" | ")
    }

    pub fn selector(type_definition: Vec<String>) -> Self {
        let hint = Self::selector_hint(&type_definition);
        Self::Selector { type_definition, hint }
    }
}

pub struct Typechecker<'a> {
    pub parsed: Parser<'a>
}

impl<'a> Typechecker<'a> {
    pub async fn new(
        parsed: Parser<'a>,
        current_path: &Path,
        mut luaurc: Option<&mut Luaurc>,
        document: &mut Document
    ) -> (Self, HashMap<PathBuf, RangeInclusive<usize>>) {
        let mut typechecker: Typechecker<'a> = Self {
            parsed
        };

        // We need to use a different ast errors
        // vec due to borrow checker issues.
        let mut ast_errors = AstErrors::new();

        let mut derives: HashMap<PathBuf, RangeInclusive<usize>> = HashMap::new();

        for datatype in &typechecker.parsed.ast {
            match datatype {
                Construct::Derive { body: Some(datatype), .. } => {
                    typechecker.typecheck_derive(datatype, &mut ast_errors, current_path, luaurc.as_deref_mut(), document, &mut derives).await;
                },

                Construct::Rule { selectors, body } => {
                    typechecker.typecheck_rule(
                        (selectors, body), &vec![], &mut ast_errors, document
                    );
                }

                _ => ()
            }
        }

        typechecker.parsed.ast_errors.0.extend(ast_errors.0);

        (typechecker, derives)
    }

    fn typecheck_derive<'b>(
        &'b self,
        body: &'b Construct<'a>,
        ast_errors: &'b mut AstErrors,
        current_path: &'b Path,
        mut luaurc: Option<&'b mut Luaurc>,
        document: &'b mut Document,
        derives: &'b mut HashMap<PathBuf, RangeInclusive<usize>>
    ) -> Pin<Box<dyn Future<Output = ()> + 'b + Send>> {
        Box::pin(async move {
            match body {
                Construct::Node {
                    node: Node {
                        token: SpannedToken(
                            span_start,
                            Token::StringSingle(content) |
                            Token::StringMulti(MultilineString { content, .. }),
                            span_end
                        ), ..
                        }
                } => {
                    self.resolve_derive(
                        content, (*span_start, *span_end), ast_errors, 
                        current_path, luaurc.as_deref_mut(), document, derives
                    ).await;
                },

                Construct::Table { body: Delimited { content, .. } } => 'table: {
                    let content = guarded_unwrap!(content.as_ref(), break 'table);

                    for item in content {
                        let datatype = 
                            if let Construct::Node { node: Node { token: SpannedToken(_, Token::SemiColon, _), .. }, .. } = item { continue }
                            else { item };
                        
                        self.typecheck_derive(&datatype, ast_errors, current_path, luaurc.as_deref_mut(), document, derives).await;
                    }
                },

                Construct::Node {
                    node: Node { token: SpannedToken(_, Token::Comma, _), .. }
                } => (),

                _ => ast_errors.push(
                    TypeError::InvalidType { expected: Some(Datatype::String) },
                    self.parsed.range_from_span(body.span())
                )
            }
        })
    }

    fn resolve_derive_alias(
        &self,
        derived_path: &str,
        current_path: &Path,
        luaurc: Option<&mut Luaurc>
    ) -> PathBuf {
        let path = 'core: {
            let derived_path = PathBuf::from(derived_path).normalize();
            let luaurc = guarded_unwrap!(luaurc, break 'core derived_path);

            let mut components = derived_path.components();

            let component = guarded_unwrap!(components.next(), break 'core derived_path);
            let component_str = component.as_os_str().to_string_lossy();

            if component_str.starts_with("@") {
                let alias = &component_str.as_ref()[1..];

                luaurc.dependants.insert(alias.to_string(), current_path.to_path_buf());

                if let Some(alias) = luaurc.aliases.get(alias) {
                    let mut derived_path = PathBuf::from(alias);

                    derived_path.push(components);

                    return derived_path
                    
                } else { derived_path }

            } else { derived_path }
        };

        current_path.join("../").join(path)
    }

    async fn resolve_derive(
        &self,
        content: &str,
        span: (usize, usize),
        ast_errors: &mut AstErrors,
        current_path: &Path,
        mut luaurc: Option<&mut Luaurc>,
        document: &mut Document,
        derives: &mut HashMap<PathBuf, RangeInclusive<usize>>
    ) {
        let mut path = self.resolve_derive_alias(content.trim(), current_path, luaurc);
        path.set_extension("rsml");

        match path.canonicalize() {
            Ok(canonicalized) => {
                if &canonicalized == current_path {
                    ast_errors.push(
                        TypeError::CyclicDerive { kind: CyclicKind::Internal },
                        self.parsed.range_from_span(span)
                    );

                } else {
                    document.dependencies.insert(canonicalized.clone());
                    document.definitions.insert(span.0..=span.1, DefinitionKind::Derive { path: canonicalized.clone() });

                    derives.insert(canonicalized, span.0..=span.1);
                }
            },

            Err(_) => {
                let normalized_path = path.normalize();

                ast_errors.push(
                    TypeError::UnknownDerive { path: Some(&normalized_path.to_string_lossy()) },
                    self.parsed.range_from_span(span)
                );

                document.definitions.insert(span.0..=span.1, DefinitionKind::Derive { path: normalized_path });
            }
        }
    }

    fn typecheck_rule(
        &self,
        (selectors, body): 
            (&Vec<Node<'a>>, &Option<Delimited<'a>>),
        parent_classes: &Vec<String>,
        ast_errors: &mut AstErrors,
        document: &mut Document
    ) {
        let current_classes = self.typecheck_selectors(
            selectors, parent_classes, ast_errors, document
        );

        let body = guarded_unwrap!(body.as_ref(), return);
        let content = guarded_unwrap!(body.content.as_ref(), return);

        for construct in content {
            match construct {
                Construct::Rule { selectors, body } => {
                    self.typecheck_rule(
                        (selectors, body), &current_classes, ast_errors, document
                    )
                },

                _ => ()
            }
        }
    }

    fn typecheck_selectors(
        &self,
        selectors: &Vec<Node<'a>>,
        parent_classes: &Vec<String>,
        ast_errors: &mut AstErrors,
        document: &mut Document
    ) -> Vec<String> {
        TypecheckSelectors::new(
            selectors, parent_classes, &self.parsed.lexer.rope, ast_errors, document
        ).classes
    }

    /*fn typecheck_selectors(
        &self,
        selectors: &Vec<Node<'a>>,
        parent_classes: &Vec<String>,
        ast_errors: &mut AstErrors,
        document: &mut Document
    ) -> Vec<String> {
        let mut selectors_iter = selectors.iter();
        let mut classes: Vec<String> = vec![];

        let mut prev_part = guarded_unwrap!(selectors_iter.next(), return classes);
        let mut part = prev_part;

        let span_start = part.token.start();

        loop {
            match part.token.value() {
                Token::Identifier(class) => {
                    let next_part = selectors_iter.next();

                    match next_part {
                        Some(next_part @ Node { token: SpannedToken(_, Token::PseudoSelector(pseudo_class), _), .. }) => {
                            self.verify_class_selector(class, &part.token, ast_errors);

                            prev_part = part;
                            part = next_part;

                            self.push_pseudo_class(pseudo_class, &mut classes, &part.token, ast_errors);
                     
                            let result =
                                self.handle_pseudo_selector(prev_part, part, &mut selectors_iter, ast_errors);
                            prev_part = result.0;
                            part = guarded_unwrap!(result.1, break);
                        },

                        Some(next_part @ Node { token: SpannedToken(_, Token::StateSelectorOrEnumPart(name), _), .. }) => {
                            self.push_class(class, &mut classes, &part.token, ast_errors);

                            prev_part = part;
                            part = next_part;

                            self.verify_state_selector(name, &part.token, ast_errors);
                     
                            let result =
                                self.handle_state_selector(prev_part, part, &mut selectors_iter, ast_errors);
                            prev_part = result.0;
                            part = guarded_unwrap!(result.1, break);
                        },

                        Some(next_part @ Node { token: SpannedToken(_, Token::Identifier(_), _), .. }) => {
                            prev_part = part;
                            part = next_part;

                            let result =
                                self.handle_class_selector("another Class", prev_part, part, &mut selectors_iter, &mut classes, ast_errors);
                            prev_part = result.0;
                            part = guarded_unwrap!(result.1, break);
                        },

                        Some(next_part @ Node { token: SpannedToken(_, Token::TagSelectorOrEnumPart(_), _), .. }) => {
                            prev_part = part;
                            part = next_part;

                            let result =
                                self.handle_tag_selector(prev_part, part, &mut selectors_iter, &mut classes, ast_errors);
                            prev_part = result.0;
                            part = guarded_unwrap!(result.1, break);
                        },

                        _ => {
                            self.push_class(class, &mut classes, &part.token, ast_errors);
                            prev_part = part;
                            part = guarded_unwrap!(next_part, break);
                        }
                    }
                },

                Token::PseudoSelector(class) => {
                    self.push_pseudo_class(class, &mut classes, &part.token, ast_errors);

                    let result =
                        self.handle_pseudo_selector(prev_part, part, &mut selectors_iter, ast_errors);
                    prev_part = result.0;
                    part = guarded_unwrap!(result.1, break);
                },

                // FOCAL POINT!!!
                Token::StateSelectorOrEnumPart(name) => {
                    classes.push(String::from("Instance"));

                    self.verify_state_selector(name, &part.token, ast_errors);

                    let result =
                        self.handle_state_selector(prev_part, part, &mut selectors_iter, ast_errors);
                    prev_part = result.0;
                    part = guarded_unwrap!(result.1, break);
                },

                Token::TagSelectorOrEnumPart(name) => {
                    classes.push(String::from("Instance"));

                    self.verify_state_selector(name, &part.token, ast_errors);

                    let result =
                        self.handle_state_selector(prev_part, part, &mut selectors_iter, ast_errors);
                    prev_part = result.0;
                    part = guarded_unwrap!(result.1, break);
                },

                _ => part = guarded_unwrap!(selectors_iter.next(), break)
            }
        }

        let span_end = prev_part.token.end();

        document.definitions.insert(span_start..=span_end, DefinitionKind::selector(classes.clone()));

        classes
    }

    fn handle_class_selector<'b>(
        &self,
        after: &str,
        mut prev_part: &'b Node<'a>,
        mut part: &'b Node<'a>,
        selectors_iter: &mut Iter<'b, Node<'a>>,
        classes: &mut Vec<String>,
        ast_errors: &mut AstErrors
    ) -> (&'b Node<'a>, Option<&'b Node<'a>>) {
        ast_errors.push(
            TypeError::InvalidSelector { msg: Some(&format!("Class Selectors can't be defined after {} Selector.", after)) },
            self.parsed.range_from_span(part.token.span())
        );

        prev_part = part;
        part = guarded_unwrap!(selectors_iter.next(), return (part, None));

        match part.token.value() {
            Token::Identifier(_) => {
                return self.handle_class_selector("another Class", prev_part, part, selectors_iter, classes, ast_errors);
            },

            Token::PseudoSelector(class) => {
                self.push_pseudo_class(class, classes, &part.token, ast_errors);

                return self.handle_pseudo_selector(prev_part, part, selectors_iter, ast_errors)
            },

            Token::StateSelectorOrEnumPart(name) => {
                self.verify_state_selector(name, &part.token, ast_errors);

                return self.handle_state_selector(prev_part, part, selectors_iter, ast_errors)
            }

            Token::TagSelectorOrEnumPart(_) | Token::NameSelector(_) => (),

            _ => ()
        };

        return (part, selectors_iter.next())
    }

    fn handle_pseudo_selector<'b>(
        &self,
        mut prev_part: &'b Node<'a>,
        mut part: &'b Node<'a>,
        selectors_iter: &mut Iter<'b, Node<'a>>,
        ast_errors: &mut AstErrors
    ) -> (&'b Node<'a>, Option<&'b Node<'a>>) {
        prev_part = part;
        part = guarded_unwrap!(selectors_iter.next(), return (part, None));

        loop {
            match part.token.value() {
                Token::PseudoSelector(_) => ast_errors.push(
                    TypeError::InvalidSelector { msg: Some("Pseudo Selectors can't be children of other Pseudo Selectors.") },
                    self.parsed.range_from_span(part.token.span())
                ),

                Token::TagSelectorOrEnumPart(_) => ast_errors.push(
                    TypeError::InvalidSelector { msg: Some("Tag Selectors can't be defined after a Pseudo Selector.") },
                    self.parsed.range_from_span(part.token.span())
                ),

                Token::NameSelector(_) => ast_errors.push(
                    TypeError::InvalidSelector { msg: Some("Name Selectors can't be defined after a Pseudo Selector.") },
                    self.parsed.range_from_span(part.token.span())
                ),

                Token::Identifier(_) => ast_errors.push(
                    TypeError::InvalidSelector { msg: Some("Class Selectors can't be defined after a Pseudo Selector.") },
                    self.parsed.range_from_span(part.token.span())
                ),

                Token::StateSelectorOrEnumPart(_) => ast_errors.push(
                    TypeError::InvalidSelector { msg: Some("State Selectors can't be defined after a Pseudo Selector.") },
                    self.parsed.range_from_span(part.token.span())
                ),

                _ => break (prev_part, Some(part))
            }

            prev_part = part;
            part = guarded_unwrap!(selectors_iter.next(), return (part, None));
        }
    }

    fn handle_state_selector<'b>(
        &self,
        mut prev_part: &'b Node<'a>,
        mut part: &'b Node<'a>,
        selectors_iter: &mut Iter<'b, Node<'a>>,
        ast_errors: &mut AstErrors
    ) -> (&'b Node<'a>, Option<&'b Node<'a>>) {
        prev_part = part;
        part = guarded_unwrap!(selectors_iter.next(), return (part, None));

        loop {
            match part.token.value() {
                Token::PseudoSelector(_) => ast_errors.push(
                    TypeError::InvalidSelector { msg: Some("Pseudo Selectors can't be defined after a State Selector.") },
                    self.parsed.range_from_span(part.token.span())
                ),

                Token::TagSelectorOrEnumPart(_) => ast_errors.push(
                    TypeError::InvalidSelector { msg: Some("Tag Selectors can't be defined after a State Selector.") },
                    self.parsed.range_from_span(part.token.span())
                ),

                Token::NameSelector(_) => ast_errors.push(
                    TypeError::InvalidSelector { msg: Some("Name Selectors can't be defined after a State Selector.") },
                    self.parsed.range_from_span(part.token.span())
                ),

                Token::Identifier(_) => ast_errors.push(
                    TypeError::InvalidSelector { msg: Some("Class Selectors can't be defined after a State Selector.") },
                    self.parsed.range_from_span(part.token.span())
                ),

                Token::StateSelectorOrEnumPart(_) => ast_errors.push(
                    TypeError::InvalidSelector { msg: Some("State Selectors can't be defined after another State Selector.") },
                    self.parsed.range_from_span(part.token.span())
                ),

                _ => break (prev_part, Some(part))
            }

            prev_part = part;
            part = guarded_unwrap!(selectors_iter.next(), return (part, None));
        }
    }

    fn handle_tag_selector<'b>(
        &self,
        mut prev_part: &'b Node<'a>,
        mut part: &'b Node<'a>,
        selectors_iter: &mut Iter<'b, Node<'a>>,
        classes: &mut Vec<String>,
        ast_errors: &mut AstErrors
    ) -> (&'b Node<'a>, Option<&'b Node<'a>>) {
        prev_part = part;
        part = guarded_unwrap!(selectors_iter.next(), return (part, None));

        match part.token.value() {
            Token::Identifier(_) => {
                return self.handle_class_selector("a Tag", prev_part, part, selectors_iter, classes, ast_errors);
            },

            Token::PseudoSelector(class) => {
                self.push_pseudo_class(class, classes, &part.token, ast_errors);

                return self.handle_pseudo_selector(prev_part, part, selectors_iter, ast_errors)
            },

            Token::StateSelectorOrEnumPart(name) => {
                self.verify_state_selector(name, &part.token, ast_errors);

                return self.handle_state_selector(prev_part, part, selectors_iter, ast_errors)
            }

            Token::TagSelectorOrEnumPart(_) | Token::NameSelector(_) => (),

            _ => ()
        };

        return (part, selectors_iter.next())
    }*/

    // Theres probably a more cleaner way to do this.
    /*fn typecheck_selectors(
        &self,
        selectors: &Vec<Node<'a>>,
        parent_selector_metadata: SelectorMetadataRef,
        ast_errors: &mut AstErrors,
        document: &mut Document
    ) -> SelectorMetadata {
        let parent_has_pseudo_selectors = parent_selector_metadata.has_pseudo_selectors;

        let mut iter = selectors.iter();

        let mut has_pseudo_selectors = false;
        let mut class_count = 0;
        let mut uses_parent_selector = false;

        let mut part = guarded_unwrap!(iter.next(), return SelectorMetadata::empty());
        let mut prev_part = part;

        let span_start = part.token.start();
        let mut current_span_start = span_start;

        let mut type_definition: Vec<Vec<String>> = vec![];
        let mut current_type_definition: Vec<String> = vec![];

        loop {
            match part.token.value() {
                Token::Identifier(class) => {
                    class_count += 1;

                    let class_passed_check =
                        self.typecheck_class(class, part.token.span(), ast_errors, None);

                    prev_part = part;
                    part = guarded_unwrap!(iter.next(), break {
                        if class_passed_check { current_type_definition.push(class.to_string()) }
                        else { current_type_definition.push(format!("!!{}!!", class)) }
                    });

                    match part.token.value() {
                        // Consumes the next node if it's a pseudo selector
                        // as its part of the current element.
                        Token::PseudoSelector(class) => {
                            has_pseudo_selectors = true;

                            self.typecheck_pseudo_class(class, part.token.span(), ast_errors, Some(&mut current_type_definition));

                            prev_part = part;
                            part = guarded_unwrap!(iter.next(), break);

                            // We need to throw errors if any Pseudo, Tag, Name or
                            // State Selectors appear after this Pseudo Selector.
                            loop {
                                match part.token.value() {
                                    Token::PseudoSelector(class) => {
                                        let span = part.token.span();

                                        self.typecheck_pseudo_class(class, span, ast_errors, None);

                                        ast_errors.push(
                                            TypeError::InvalidSelector { msg: Some("Pseudo Selectors can't be children of other Pseudo Selectors.") },
                                            self.parsed.range_from_span(span)
                                        );
                                    },

                                    Token::TagSelectorOrEnumPart(_) => ast_errors.push(
                                        TypeError::InvalidSelector { msg: Some("Tag Selectors can't be defined after a Pseudo Selector.") },
                                        self.parsed.range_from_span(part.token.span())
                                    ),

                                    Token::NameSelector(_) => ast_errors.push(
                                        TypeError::InvalidSelector { msg: Some("Name Selectors can't be defined after a Pseudo Selector.") },
                                        self.parsed.range_from_span(part.token.span())
                                    ),

                                    Token::StateSelectorOrEnumPart(_) => ast_errors.push(
                                        TypeError::InvalidSelector { msg: Some("State Selectors can't be defined after a Pseudo Selector.") },
                                        self.parsed.range_from_span(part.token.span())
                                    ),

                                    _ => break
                                }

                                prev_part = part;
                                part = guarded_unwrap!(iter.next(), break);
                            }
                        },

                        Token::StateSelectorOrEnumPart(name) => {
                            self.typecheck_state_selector(name, part.token.span(), ast_errors);

                            prev_part = part;
                            part = guarded_unwrap!(iter.next(), break);
                        }

                        _ => {
                            if class_passed_check { current_type_definition.push(class.to_string()) }
                            else {current_type_definition.push(format!("!!{}!!", class)) }
                        }
                    }
                },

                Token::PseudoSelector(class) => {
                    has_pseudo_selectors = true;

                    let span = part.token.span();

                    self.typecheck_pseudo_class(class, span, ast_errors, Some(&mut current_type_definition));

                    if parent_has_pseudo_selectors {
                        ast_errors.push(
                            TypeError::InvalidSelector { msg: Some("Pseudo Selectors can't be children of other Pseudo Selectors.") },
                            self.parsed.range_from_span(span)
                        );
                    }

                    prev_part = part;
                    part = guarded_unwrap!(iter.next(), break);

                    if let Token::Identifier(class) = part.token.value() {
                        class_count += 2;

                        self.typecheck_class(class,  part.token.span(), ast_errors, Some(&mut current_type_definition));

                        prev_part = part;
                        part = guarded_unwrap!(iter.next(), break);
                    }
                },

                Token::ChildrenSelector | Token::DescendantsSelector => {
                    prev_part = part;
                    part = guarded_unwrap!(iter.next(), break);

                    if let Token::StateSelectorOrEnumPart(name) = part.token.value() {
                        self.typecheck_state_selector(name, part.token.span(), ast_errors);

                        prev_part = part;
                        part = guarded_unwrap!(iter.next(), break);
                    }
                },

                Token::StateSelectorOrEnumPart(name) => {
                    self.typecheck_state_selector(name, part.token.span(), ast_errors);

                    if let Some(parent_type_definition) = parent_selector_metadata.type_definition {
                        type_definition.extend_from_slice(parent_type_definition);
                        uses_parent_selector = true;
                    }

                    prev_part = part;
                    part = guarded_unwrap!(iter.next(), break);
                }

                Token::Comma => {
                    if class_count >= 1 {
                        type_definition.push(current_type_definition);
                        current_type_definition = vec![]
                    }

                    if class_count > 1 {
                        class_count = 0;

                        let current_span_end = prev_part.token.end();

                        ast_errors.push(
                            TypeError::InvalidSelector { msg: Some("Matching more than one class on the same element is impossible.") },
                            self.parsed.range_from_span((current_span_start, current_span_end))
                        );

                        prev_part = part;
                        part = guarded_unwrap!(iter.next(), break);
                        current_span_start = part.token.start();

                    } else {
                        class_count = 0;

                        prev_part = part;
                        part = guarded_unwrap!(iter.next(), break);
                    }
                },

                _ => {
                    prev_part = part;
                    part = guarded_unwrap!(iter.next(), break);
                }
            }
        }

        if class_count >= 1 {
            type_definition.push(current_type_definition);
        }

        let span_end = prev_part.token.end();

        if type_definition.len() == 0 {
            type_definition.push(vec!["Instance".to_string()]);
        }

        document.definitions.insert(span_start..=span_end, DefinitionKind::selector(type_definition.clone()));

        if class_count > 1 || (uses_parent_selector && parent_selector_metadata.class_count > 1) {
            ast_errors.push(
                TypeError::InvalidSelector { msg: Some("Matching more than one class on the same element is impossible.") },
                self.parsed.range_from_span((current_span_start, span_end))
            );
        };

        SelectorMetadata {
            type_definition: Some(type_definition),
            has_pseudo_selectors,
            class_count
        }
    }*/

    /*fn verify_state_selector<'b>(
        &self,
        name: &'b str,
        token: &SpannedToken,
        ast_errors: &mut AstErrors
    ) -> bool {
        if ALLOWED_STATE_SELECTORS.contains(name) { return true }

        ast_errors.push(
            TypeError::InvalidSelector { msg: Some(&format!("No state named \"{}\" exists.", name)) },
            self.parsed.range_from_span(token.span())
        );

        false
    }

    fn verify_class_selector<'b>(
        &self,
        class: &'b str,
        token: &SpannedToken,
        ast_errors: &mut AstErrors
    ) -> bool {
        if rbx_reflection_database::get().classes.contains_key(class) {
            return true
        }

        ast_errors.push(
            TypeError::InvalidSelector { msg: Some(&format!("No class named \"{}\" exists.", class)) },
            self.parsed.range_from_span(token.span())
        );

        false
    }

    fn push_class<'b>(
        &self,
        class: &'b str,
        classes: &mut Vec<String>,
        token: &SpannedToken,
        ast_errors: &mut AstErrors
    ) -> bool {
        if rbx_reflection_database::get().classes.contains_key(class) {
            classes.push(class.to_string());
            return true
        }

        classes.push(String::from("Instance"));

        ast_errors.push(
            TypeError::InvalidSelector { msg: Some(&format!("No class named \"{}\" exists.", class)) },
            self.parsed.range_from_span(token.span())
        );

        return false
    }

    fn push_pseudo_class<'b>(
        &self,
        class: &'b str,
        classes: &mut Vec<String>,
        token: &SpannedToken,
        ast_errors: &mut AstErrors
    ) -> bool {
        if !rbx_reflection_database::get().classes.contains_key(class) {
            classes.push(String::from("Instance"));

            ast_errors.push(
                TypeError::InvalidSelector { msg: Some(&format!("No class named \"{}\" exists.", class)) },
                self.parsed.range_from_span(token.span())
            );

            return false
        }

        if !ALLOWED_PSEUDO_SELECTORS.contains(class) {
            classes.push(class.to_string());

            ast_errors.push(
                TypeError::InvalidSelector { msg: Some(&format!("Class \"{}\" can't be used as a Pseudo instance.", class)) },
                self.parsed.range_from_span(token.span())
            );

            return false
        }

        classes.push(class.to_string());
 
        return true
    }*/
}


static ALLOWED_PSEUDO_SELECTORS: phf::Set<&str> = phf_set! {
    "UICorner",
    "UIGradient",
    "UIPadding",
    "UIStroke",
    "UIListLayout",
    "UIGridStyleLayout",
    "UIGridLayout",
    "UIPageLayout",
    "UIAspectRatioConstraint",
    "UISizeConstraint",
    "UITextSizeConstraint",
    "UIScale",
    "UIFlexItem"
};

static ALLOWED_STATE_SELECTORS: phf::Set<&str> = phf_set! {
    "idle",
    "hover",
    "press",
    "pressed",
    "noninteractable"
};



struct TypecheckSelectors<'a> {
    iter: Iter<'a, Node<'a>>,
    parent_classes: &'a Vec<String>,
    classes: Vec<String>,

    part: Option<&'a Node<'a>>,
    has_name: bool,

    rope: &'a Rope,
    ast_errors: &'a mut AstErrors
}

impl<'a> TypecheckSelectors<'a> {
    fn new(
        selectors: &'a Vec<Node<'a>>,
        parent_classes: &'a Vec<String>,
        rope: &'a Rope,
        ast_errors: &'a mut AstErrors,
        document: &mut Document
    ) -> Self {
        let mut typecheck_selectors = Self {
            iter: selectors.iter(),
            parent_classes,
            classes: Vec::new(),
            part: None,
            has_name: false,
            rope,
            ast_errors
        };

        typecheck_selectors.begin(document);

        typecheck_selectors
    }

    fn next(&mut self) -> Option<&'a Node<'a>> {
        let Some(next_part) = self.iter.next() else { return None };

        self.part = Some(next_part);

        Some(next_part)
    }

    fn begin_iteration(&mut self, part: &'a Node<'a>) {
        if self.parent_classes.is_empty() {
            self.from_new(part);

        } else {
            self.from_parent(part);
        }
    }

    fn begin(&mut self, document: &mut Document) {
        let Some(part) = self.next() else { return };
        let span_start = part.token.start();

        self.begin_iteration(part);

        let span_end = self.part
            .map(|x| x.token.end())
            .unwrap_or_else(|| part.token.end());

        document.definitions.insert(span_start..=span_end, DefinitionKind::selector(self.classes.clone()));
    }

    fn from_new(&mut self, part: &'a Node<'a>) {
        match part.token.value() {
            Token::Identifier(class) => {
                let validated_class = self.validate_class(class, &part.token);

                match self.consume_with_error(
                    TokenKind::Identifier,
                    token_kind_list![ PseudoSelector, StateSelectorOrEnumPart ],
                    Some(token_kind_list![ TagSelectorOrEnumPart, NameSelector ])
                ) {
                    ConsumeResult::Some(part) => {
                        match part.token.value() {
                            Token::PseudoSelector(class) => {
                                let validated_class = self.validate_psuedo_class(class, &part.token);
                                self.classes.push(validated_class.to_string());
                            },

                            Token::StateSelectorOrEnumPart(class) => {
                                self.classes.push(validated_class.to_string());

                                self.validate_state(class, &part.token);
                            },

                            _ => ()
                        }
                    },

                    ConsumeResult::Err(_) => {
                        self.classes.push(validated_class.to_string());

                        let Some(part) = self.next() else { return };
                        self.begin_iteration(part);
                    },

                    ConsumeResult::None => {
                        self.classes.push(validated_class.to_string());
                    }
                }
            },

            _ => ()
        }
    }

    fn from_parent(&mut self, part: &'a Node<'a>) {

    }

    fn consume_with_error<const N: usize>(
        &mut self,
        origin_kind: TokenKind,
        allow_list: &TokenKindList<N>,
        error_exclude_list: Option<&TokenKindList<N>>
    ) -> ConsumeResult<'a> {
        self.consume(allow_list, |checker, part| checker.error(
                error_exclude_list,
                origin_kind,
                part.token.value().kind(),
                part.token.span()
            )
        )
    }

    fn consume<const N: usize, F: FnMut(&mut TypecheckSelectors<'a>, &'a Node<'a>) -> ()>(
        &mut self,
        allow_list: &TokenKindList<N>,
        mut error_callback: F
    ) -> ConsumeResult<'a> {
        while let Some(part) = self.next() {
            let token = part.token.value();
            let token_discriminant = token.discriminant();

            if allow_list.has_discriminant(&token_discriminant) {
                return ConsumeResult::Some(part)

            } else if matches!(token, Token::Comma | Token::ChildrenSelector | Token::DescendantsSelector) {
                return ConsumeResult::Err(part)

            } else {
                error_callback(self, part)
            }
        };

        ConsumeResult::None
    }

    fn error<const N: usize>(
        &mut self,
        error_exclude_list: Option<&TokenKindList<N>>,
        origin_kind: TokenKind,
        subject_kind: TokenKind,
        subject_span: (usize, usize)
    ) {
        if let Some(error_exclude_list) = error_exclude_list &&
            error_exclude_list.has_discriminant(&discriminant(&subject_kind)) { return }

        let origin_selector_name = self.selector_name(origin_kind);

        if origin_kind == subject_kind {
            self.ast_errors.push(
                TypeError::InvalidSelector {
                    msg: Some(&format!("{} Selectors can't be defined after another {} Selector.", origin_selector_name, origin_selector_name))
                },
                self.range_from_span(subject_span)
            )

        } else {
            let subject_selector_name = self.selector_name(subject_kind);

            self.ast_errors.push(
                TypeError::InvalidSelector {
                    msg: Some(&format!("{} Selectors can't be defined after a {} Selector.", origin_selector_name, subject_selector_name))
                },
                self.range_from_span(subject_span)
            )
        }
    }

    fn selector_name(&self, kind: TokenKind) -> &'static str {
        match kind {
            TokenKind::Identifier => "Class",
            TokenKind::TagSelectorOrEnumPart => "Tag",
            TokenKind::StateSelectorOrEnumPart => "State",
            TokenKind::NameSelector => "Name",
            TokenKind::ChildrenSelector => "Children",
            TokenKind::DescendantsSelector => "Descendants",
            _ => "Unknown"
        }
    }

    /// Returns the class if it valid, if its invalid then it returns `"Instance"`.
    fn validate_class<'b>(
        &mut self,
        class: &'a str,
        token: &SpannedToken
    ) -> &'a str {
        if rbx_reflection_database::get().classes.contains_key(class) {
            return class
        }

        self.ast_errors.push(
            TypeError::InvalidSelector { msg: Some(&format!("No class named \"{}\" exists.", class)) },
            self.range_from_span(token.span())
        );

        "Instance"
    }

    fn validate_psuedo_class(
        &mut self,
        class: &'a str,
        token: &SpannedToken
    ) -> &'a str {
        if !rbx_reflection_database::get().classes.contains_key(class) {
            self.ast_errors.push(
                TypeError::InvalidSelector { msg: Some(&format!("No class named \"{}\" exists.", class)) },
                self.range_from_span(token.span())
            );

            return "Instance"
        }

        if !ALLOWED_PSEUDO_SELECTORS.contains(class) {
            self.ast_errors.push(
                TypeError::InvalidSelector { msg: Some(&format!("Class \"{}\" can't be used as a Pseudo instance.", class)) },
                self.range_from_span(token.span())
            );
        }

        return class
    }

    fn validate_state(
        &mut self,
        name: &'a str,
        token: &SpannedToken
    ) -> bool {
        if ALLOWED_STATE_SELECTORS.contains(name) { return true }

        self.ast_errors.push(
            TypeError::InvalidSelector { msg: Some(&format!("No state named \"{}\" exists.", name)) },
            self.range_from_span(token.span())
        );

        false
    }

    fn range_from_span(&self, span: (usize, usize)) -> Range {
        Range::from_span(&self.rope, span)
    }
}


enum ConsumeResult<'a> {
    Some(&'a Node<'a>),
    None,
    Err(&'a Node<'a>)
}