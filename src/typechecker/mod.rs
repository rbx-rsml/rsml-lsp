use std::{ops::{Deref, DerefMut}, path::PathBuf};

use crate::{guarded_unwrap, lexer::{MultilineString, SpannedToken, Token}, luaurc::Luaurc, normalize_path::NormalizePath, parser::{AstErrors, Construct, Delimited, Node, Parser}, typechecker::type_error::{CyclicKind, Datatype}, Document, Workspaces};

mod type_error;
use phf_macros::phf_set;
use rangemap::{RangeInclusiveMap};
use tower_lsp::lsp_types::{Diagnostic, NumberOrString, Range};
use type_error::TypeError;

trait PushTypeError {
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
        type_definition: Vec<Vec<String>>,
        hint: String
    }
}

impl DefinitionKind {
    fn selector_hint(type_definition: &Vec<Vec<String>>) -> String {
        let mut iter = type_definition.iter();

        let mut next = guarded_unwrap!(iter.next(), return String::new());

        let mut hint =
            if next.len() == 1 { next.join(" & ") }
            else { format!("({})", next.join(" & ")) };

        next = guarded_unwrap!(iter.next(), return hint);

        loop {
            hint += &format!(
                " | {}",
                if next.len() == 1 { next.join(" & ") }
                else { format!("({})", next.join(" & ")) }
            );

            next = guarded_unwrap!(iter.next(), return hint);
        }
    }

    pub fn selector(type_definition: Vec<Vec<String>>) -> Self {
        let hint = Self::selector_hint(&type_definition);
        Self::Selector { type_definition, hint }
    }
}

pub struct Typechecker<'a> {
    pub parsed: Parser<'a>
}

impl<'a> Typechecker<'a> {
    pub fn new(
        parsed: Parser<'a>,
        current_path: &PathBuf,
        workspaces: &mut Workspaces,
        document: &mut Document,
        luaurc: Option<&Luaurc>
    ) -> Self {
        let mut typechecker = Self {
            parsed
        };

        // We need to use a different ast errors
        // vec due to borrow checker issues.
        let mut ast_errors = AstErrors::new();

        for datatype in &typechecker.parsed.ast {
            match datatype {
                Construct::Derive { body: Some(datatype), .. } =>
                    typechecker.typecheck_derive(datatype, &mut ast_errors, current_path, workspaces, document, luaurc),

                Construct::Rule { selectors, body } =>
                    typechecker.typecheck_rule((selectors, body), false, &mut ast_errors, document),

                _ => ()
            }
        }

        typechecker.parsed.ast_errors.0.extend(ast_errors.0);

        typechecker
    }

    fn typecheck_derive<'b>(
        &'b self,
        body: &'b Construct<'a>,
        ast_errors: &'b mut AstErrors,
        current_path: &'b PathBuf,
        workspaces: &'b mut Workspaces,
        document: &'b mut Document,
        luaurc: Option<&'b Luaurc>
    ) {
        match body {
            Construct::Node {
                node: Node {
                    token: SpannedToken(
                        span_start,
                        Token::StringSingle(content) |
                        Token::StringMulti(MultilineString { content, .. }),
                        span_end), ..
                    }
            } => {
                self.resolve_derive(
                    content, (*span_start, *span_end), ast_errors, 
                    current_path, document, luaurc
                );
            },

            Construct::Table { body: Delimited { content, .. } } => 'table: {
                let content = guarded_unwrap!(content.as_ref(), break 'table);

                for item in content {
                    let datatype = 
                        if let Construct::Node { node: Node { token: SpannedToken(_, Token::SemiColon, _), .. }, .. } = item { continue }
                        else { item };
                    
                    self.typecheck_derive(&datatype, ast_errors, current_path, workspaces, document, luaurc);
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
    }

    fn resolve_derive_alias(
        &self,
        path_str: &str,
        current_path: &PathBuf,
        luaurc: Option<&Luaurc>
    ) -> PathBuf {
        let path = 'core: {
            let path = PathBuf::from(path_str).normalize();
            let luaurc = guarded_unwrap!(luaurc, break 'core path);

            let mut components = path.components();

            let component = guarded_unwrap!(components.next(), break 'core path);
            let component_str = component.as_os_str().to_string_lossy();

            if component_str.starts_with("@") &&
                let Some(alias) = luaurc.aliases.get(&component_str.as_ref()[1..])
            {
                let mut path = PathBuf::from(alias);

                path.push(components);

                return path
            } else { path }
        };

        current_path.join("../").join(path)
    }

    fn resolve_derive(
        &self,
        content: &str,
        span: (usize, usize),
        ast_errors: &mut AstErrors,
        current_path: &PathBuf,
        document: &mut Document,
        luaurc: Option<&Luaurc>
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
                    document.definitions.insert(span.0..=span.1, DefinitionKind::Derive { path: canonicalized });
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
        parent_has_psuedo_selector: bool,
        ast_errors: &mut AstErrors,
        document: &mut Document
    ) {
        let current_has_psuedo_selector =
            self.typecheck_selectors(selectors, parent_has_psuedo_selector, ast_errors, document);

        let body = guarded_unwrap!(body.as_ref(), return);
        let content = guarded_unwrap!(body.content.as_ref(), return);

        for construct in content {
            match construct {
                Construct::Rule { selectors, body } =>
                    self.typecheck_rule((selectors, body), current_has_psuedo_selector, ast_errors, document),

                _ => ()
            }
        }
    }

    fn typecheck_selectors(
        &self,
        selectors: &Vec<Node<'a>>,
        parent_has_psuedo_selector: bool,
        ast_errors: &mut AstErrors,
        document: &mut Document
    ) -> bool {
        let mut iter = selectors.iter();

        let mut current_has_psuedo_selector = false;

        let mut part = guarded_unwrap!(iter.next(), return false);
        let mut prev_part = part;

        let span_start = part.token.start();
        let mut current_span_start = span_start;

        let mut class_count = 0;

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

                    // Consumes the next node if it's a psuedo selector
                    // as its part of the current element.
                    if let Token::PseudoSelector(class) = part.token.value() {
                        current_has_psuedo_selector = true;

                        self.typecheck_psuedo_class(class, part.token.span(), ast_errors, Some(&mut current_type_definition));

                        prev_part = part;
                        part = guarded_unwrap!(iter.next(), break);

                        // We need to throw errors if any Psuedo, Tag, Name or
                        // State Selectors appear after this Psuedo Selector.
                        loop {
                            match part.token.value() {
                                Token::PseudoSelector(class) => {
                                    let span = part.token.span();

                                    self.typecheck_psuedo_class(class, span, ast_errors, None);

                                    ast_errors.push(
                                        TypeError::InvalidSelector { msg: Some("Psuedo Selectors can't be children of other Psuedo Selectors.") },
                                        self.parsed.range_from_span(span)
                                    );
                                },

                                Token::TagSelectorOrEnumPart(_) => ast_errors.push(
                                    TypeError::InvalidSelector { msg: Some("Tag Selectors can't be defined after a Psuedo Selector.") },
                                    self.parsed.range_from_span(part.token.span())
                                ),

                                Token::NameSelector(_) => ast_errors.push(
                                    TypeError::InvalidSelector { msg: Some("Name Selectors can't be defined after a Psuedo Selector.") },
                                    self.parsed.range_from_span(part.token.span())
                                ),

                                Token::StateSelectorOrEnumPart(_) => ast_errors.push(
                                    TypeError::InvalidSelector { msg: Some("State Selectors can't be defined after a Psuedo Selector.") },
                                    self.parsed.range_from_span(part.token.span())
                                ),

                                _ => break
                            }

                            prev_part = part;
                            part = guarded_unwrap!(iter.next(), break);
                        }

                    } else {
                        if class_passed_check { current_type_definition.push(class.to_string()) }
                        else {current_type_definition.push(format!("!!{}!!", class)) }
                    }
                },

                Token::PseudoSelector(class) => {
                    current_has_psuedo_selector = true;

                    let span = part.token.span();

                    self.typecheck_psuedo_class(class, span, ast_errors, Some(&mut current_type_definition));

                    if parent_has_psuedo_selector {
                        ast_errors.push(
                            TypeError::InvalidSelector { msg: Some("Psuedo Selectors can't be children of other Psuedo Selectors.") },
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

                Token::StateSelectorOrEnumPart(name) => {
                    self.typecheck_state_selector(name, part.token.span(), ast_errors);

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

        document.definitions.insert(span_start..=span_end, DefinitionKind::selector(type_definition));

        if class_count > 1 {
            ast_errors.push(
                TypeError::InvalidSelector { msg: Some("Matching more than one class on the same element is impossible.") },
                self.parsed.range_from_span((current_span_start, span_end))
            );
        };

        current_has_psuedo_selector
    }

    fn typecheck_class<'b>(
        &self,
        class: &'b str,
        span: (usize, usize),
        ast_errors: &mut AstErrors,
        current_type_definition: Option<&mut Vec<String>>
    ) -> bool {
        if rbx_reflection_database::get().classes.contains_key(class) {
            if let Some(current_type_definition) = current_type_definition {
                current_type_definition.push(class.to_string());
            }

            return true
        }

        ast_errors.push(
            TypeError::InvalidSelector { msg: Some(&format!("No class named \"{}\" exists.", class)) },
            self.parsed.range_from_span(span)
        );

        if let Some(current_type_definition) = current_type_definition {
            current_type_definition.push(format!("!!{}!!", class));
        }

        return false
    }

    fn typecheck_psuedo_class<'b>(
        &self,
        class: &'b str,
        span: (usize, usize),
        ast_errors: &mut AstErrors,
        current_type_definition: Option<&mut Vec<String>>
    ) {
        // We add 2 to the start span to accomodate for the `::` prefix.
        if !self.typecheck_class(class, (span.0 + 2, span.1), ast_errors, current_type_definition) { return };

        if ALLOWED_PSEUDO_SELECTORS.contains(class) { return };

        ast_errors.push(
            TypeError::InvalidSelector { msg: Some(&format!("Class \"{}\" is not allowed as a Pseudo Selector.", class)) },
            self.parsed.range_from_span(span)
        );
    }

    fn typecheck_state_selector(&self, name: &str, span: (usize, usize), ast_errors: &mut AstErrors) {
        if ALLOWED_STATE_SELECTORS.contains(&name.to_lowercase()) { return };

        ast_errors.push(
            TypeError::InvalidSelector { msg: Some(&format!("Unknown state \"{}\".", name)) },
            self.parsed.range_from_span(span)
        );
    }
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


