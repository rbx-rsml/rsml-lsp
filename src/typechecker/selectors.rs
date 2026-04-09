use std::{mem::discriminant, slice::Iter};

use indexmap::IndexSet;

use crate::{
    Document,
    lexer::{SpannedToken, Token, TokenKind},
    list::TokenKindList,
    parser::{AstErrors, Construct, Delimited, Node},
    range_from_span::RangeFromSpan,
    token_kind_list,
};

use phf_macros::phf_set;
use ropey::Rope;
use tower_lsp::lsp_types::Range;

use super::{DefinitionKind, PushTypeError, Typechecker, type_error::*};

impl<'a> Typechecker<'a> {
    pub(super) fn typecheck_rule(
        &self,
        (selectors, body): (&Option<Vec<Node<'a>>>, &Option<Delimited<'a>>),
        parent_classes: &Vec<String>,
        ast_errors: &mut AstErrors,
        document: &mut Document,
    ) {
        let current_classes = if let Some(selectors) = selectors {
            self.typecheck_selectors(selectors, parent_classes, ast_errors, document)
        } else {
            parent_classes.clone()
        };

        let Some(body) = body.as_ref() else { return };

        let body_start = body.left.token.start();
        let body_end = body.right.as_ref()
            .map(|r| r.token.end())
            .unwrap_or(body.left.token.end());
        document.definitions.insert(
            body_start..=body_end,
            DefinitionKind::Scope { type_definition: current_classes.clone() },
        );

        let Some(content) = body.content.as_ref() else {
            return;
        };

        for construct in content {
            match construct {
                Construct::Rule { selectors, body } => {
                    self.typecheck_rule((selectors, body), &current_classes, ast_errors, document)
                }

                Construct::Assignment { left, middle, right, terminator } => {
                    let Token::Identifier(property_name) = left.token.value() else { continue };
                    let Some(middle) = middle else { continue };

                    let assign_start = middle.token.start();
                    let assign_end = terminator.as_ref()
                        .map(|t| t.token.end())
                        .or_else(|| right.as_ref().map(|r| r.span().1))
                        .unwrap_or(middle.token.end());

                    document.definitions.insert(
                        assign_start..=assign_end,
                        DefinitionKind::Assignment {
                            property_name: property_name.to_string(),
                            type_definition: current_classes.clone(),
                        },
                    );

                    let Some(right) = right else { continue };
                    match right.as_ref() {
                        Construct::Enum { keyword, name, variant } => {
                            let name_range_start = keyword.token.end();
                            let name_range_end = name.as_ref()
                                .map(|node| node.token.end())
                                .unwrap_or(assign_end);

                            document.definitions.insert(
                                name_range_start..=name_range_end,
                                DefinitionKind::EnumName,
                            );

                            if let Some(name_node) = name {
                                let enum_name = match name_node.token.value() {
                                    Token::TagSelectorOrEnumPart(name) => name,
                                    Token::StateSelectorOrEnumPart(name) => name,
                                    _ => continue,
                                };

                                let variant_range_start = name_node.token.end();
                                let variant_range_end = variant.as_ref()
                                    .map(|node| node.token.end())
                                    .unwrap_or(assign_end);

                                document.definitions.insert(
                                    variant_range_start..=variant_range_end,
                                    DefinitionKind::EnumVariant {
                                        enum_name: enum_name.to_string(),
                                    },
                                );
                            }
                        }

                        _ => (),
                    }
                }

                _ => (),
            }
        }
    }

    fn typecheck_selectors(
        &self,
        selectors: &Vec<Node<'a>>,
        parent_classes: &Vec<String>,
        ast_errors: &mut AstErrors,
        document: &mut Document,
    ) -> Vec<String> {
        TypecheckSelectors::new(
            selectors,
            parent_classes,
            &self.parsed.lexer.rope,
            ast_errors,
            document,
        )
        .classes
        .into_iter()
        .collect()
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

struct TypecheckSelectors<'a> {
    iter: Iter<'a, Node<'a>>,
    parent_classes: &'a Vec<String>,
    classes: IndexSet<String>,

    part: Option<&'a Node<'a>>,
    has_name: bool,

    rope: &'a Rope,
    ast_errors: &'a mut AstErrors,
}

impl<'a> TypecheckSelectors<'a> {
    fn new(
        selectors: &'a Vec<Node<'a>>,
        parent_classes: &'a Vec<String>,
        rope: &'a Rope,
        ast_errors: &'a mut AstErrors,
        document: &mut Document,
    ) -> Self {
        let mut typecheck_selectors = Self {
            iter: selectors.iter(),
            parent_classes,
            classes: IndexSet::new(),
            part: None,
            has_name: false,
            rope,
            ast_errors,
        };

        typecheck_selectors.begin(document);

        typecheck_selectors
    }

    fn next(&mut self) -> Option<&'a Node<'a>> {
        let next_part = self.iter.next()?;
        self.part = Some(next_part);
        Some(next_part)
    }

    fn begin_iteration(&mut self, part: &'a Node<'a>) {
        if self.parent_classes.is_empty() {
            self.from_new(part);
        } else {
            self.from_parent(part, false);
        }
    }

    fn begin(&mut self, document: &mut Document) {
        let Some(part) = self.next() else { return };
        let span_start = part.token.start();

        self.begin_iteration(part);

        let span_end = self
            .part
            .map(|x| x.token.end())
            .unwrap_or_else(|| part.token.end());

        document.definitions.insert(
            span_start..=span_end,
            DefinitionKind::selector(self.classes.iter().cloned().collect()),
        );
    }

    fn from_new(&mut self, part: &'a Node<'a>) {
        match part.token.value() {
            Token::TagSelectorOrEnumPart(_) | Token::NameSelector(_) => {
                self.classes.insert("Instance".to_string());
                self.consume_past_comma();
            }

            Token::Identifier(class) => {
                let validated_class = self.validate_class(class, &part.token);

                match self.consume_with_error(
                    TokenKind::Identifier,
                    token_kind_list![PseudoSelector, StateSelectorOrEnumPart],
                    Some(token_kind_list![TagSelectorOrEnumPart, NameSelector]),
                ) {
                    ConsumeResult::Some(part) => match part.token.value() {
                        Token::PseudoSelector(class) => {
                            let validated_class = self.validate_pseudo_class(class, &part.token);
                            self.classes.insert(validated_class.to_string());
                        }

                        Token::StateSelectorOrEnumPart(class) => {
                            self.classes.insert(validated_class.to_string());

                            self.validate_state(class, &part.token);
                        }

                        _ => (),
                    },

                    ConsumeResult::Err(delimiter) => {
                        if matches!(delimiter.token.value(), Token::Comma) {
                            self.classes.insert(validated_class.to_string());
                        }

                        let Some(part) = self.next() else { return };
                        self.begin_iteration(part);
                    }

                    ConsumeResult::None => {
                        self.classes.insert(validated_class.to_string());
                    }
                }
            }

            _ => (),
        }
    }

    fn from_parent(&mut self, part: &'a Node<'a>, after_combinator: bool) {
        match part.token.value() {
            Token::Identifier(class) => {
                if !after_combinator {
                    self.ast_errors.push(
                        TypeError::InvalidSelector {
                            msg: Some("Class Selectors can't be nested inside another selector without a children (>) or descendants selector."),
                        },
                        self.range_from_span(part.token.span()),
                    );
                }

                let validated_class = self.validate_class(class, &part.token);
                self.classes.insert(validated_class.to_string());

                match self.consume_with_error(
                    TokenKind::Identifier,
                    token_kind_list![PseudoSelector, StateSelectorOrEnumPart],
                    Some(token_kind_list![TagSelectorOrEnumPart, NameSelector]),
                ) {
                    ConsumeResult::Some(part) => match part.token.value() {
                        Token::PseudoSelector(class) => {
                            self.classes.pop();
                            let validated_class = self.validate_pseudo_class(class, &part.token);
                            self.classes.insert(validated_class.to_string());
                        }

                        Token::StateSelectorOrEnumPart(state) => {
                            self.validate_state(state, &part.token);
                        }

                        _ => (),
                    },

                    ConsumeResult::Err(_) => {
                        let Some(part) = self.next() else { return };
                        self.begin_iteration(part);
                    }

                    ConsumeResult::None => (),
                }
            }

            Token::PseudoSelector(class) => {
                let validated_class = self.validate_pseudo_class(class, &part.token);
                self.classes.insert(validated_class.to_string());
            }

            Token::StateSelectorOrEnumPart(state) => {
                self.classes.extend(self.parent_classes.iter().cloned());
                self.validate_state(state, &part.token);
            }

            Token::TagSelectorOrEnumPart(_) | Token::NameSelector(_) => {
                self.classes.insert("Instance".to_string());
                self.consume_past_comma();
            }

            Token::ChildrenSelector | Token::DescendantsSelector => {
                let Some(next) = self.next() else { return };
                self.from_parent(next, true);
            }

            _ => (),
        }
    }

    fn consume_past_comma(&mut self) {
        let Some(part) = self.next() else { return };
        if matches!(part.token.value(), Token::Comma) {
            let Some(next) = self.next() else { return };
            self.begin_iteration(next);
        }
    }

    fn consume_with_error<const N: usize>(
        &mut self,
        origin_kind: TokenKind,
        allow_list: &TokenKindList<N>,
        error_exclude_list: Option<&TokenKindList<N>>,
    ) -> ConsumeResult<'a> {
        self.consume(allow_list, |checker, part| {
            checker.error(
                error_exclude_list,
                origin_kind,
                part.token.value().kind(),
                part.token.span(),
            )
        })
    }

    fn consume<const N: usize, F: FnMut(&mut TypecheckSelectors<'a>, &'a Node<'a>) -> ()>(
        &mut self,
        allow_list: &TokenKindList<N>,
        mut error_callback: F,
    ) -> ConsumeResult<'a> {
        while let Some(part) = self.next() {
            let token = part.token.value();
            let token_discriminant = token.discriminant();

            if allow_list.has_discriminant(&token_discriminant) {
                return ConsumeResult::Some(part);
            } else if matches!(
                token,
                Token::Comma | Token::ChildrenSelector | Token::DescendantsSelector
            ) {
                return ConsumeResult::Err(part);
            } else {
                error_callback(self, part)
            }
        }

        ConsumeResult::None
    }

    fn error<const N: usize>(
        &mut self,
        error_exclude_list: Option<&TokenKindList<N>>,
        origin_kind: TokenKind,
        subject_kind: TokenKind,
        subject_span: (usize, usize),
    ) {
        if let Some(error_exclude_list) = error_exclude_list
            && error_exclude_list.has_discriminant(&discriminant(&subject_kind))
        {
            return;
        }

        let origin_name = self.selector_name(origin_kind);
        let subject_name = self.selector_name(subject_kind);
        let msg = if origin_kind == subject_kind {
            format!(
                "{origin_name} Selectors can't be defined after another {origin_name} Selector without a children (>) or descendants selector."
            )
        } else {
            format!("{origin_name} Selectors can't be defined after a {subject_name} Selector without a children (>) or descendants selector.")
        };

        self.ast_errors.push(
            TypeError::InvalidSelector { msg: Some(&msg) },
            self.range_from_span(subject_span),
        );
    }

    fn selector_name(&self, kind: TokenKind) -> &'static str {
        match kind {
            TokenKind::Identifier => "Class",
            TokenKind::TagSelectorOrEnumPart => "Tag",
            TokenKind::StateSelectorOrEnumPart => "State",
            TokenKind::NameSelector => "Name",
            TokenKind::ChildrenSelector => "Children",
            TokenKind::DescendantsSelector => "Descendants",
            _ => "Unknown",
        }
    }

    /// Returns the class if it valid, if its invalid then it returns `"Instance"`.
    fn validate_class<'b>(&mut self, class: &'a str, token: &SpannedToken) -> &'a str {
        if rbx_reflection_database::get().classes.contains_key(class) {
            return class;
        }

        self.ast_errors.push(
            TypeError::InvalidSelector {
                msg: Some(&format!("No class named \"{}\" exists.", class)),
            },
            self.range_from_span(token.span()),
        );

        "Instance"
    }

    fn validate_pseudo_class(&mut self, class: &'a str, token: &SpannedToken) -> &'a str {
        let validated = self.validate_class(class, token);

        if validated != "Instance" && !ALLOWED_PSEUDO_SELECTORS.contains(class) {
            self.ast_errors.push(
                TypeError::InvalidSelector {
                    msg: Some(&format!(
                        "Class \"{}\" can't be used as a Pseudo instance.",
                        class
                    )),
                },
                self.range_from_span(token.span()),
            );
        }

        validated
    }

    fn validate_state(&mut self, name: &'a str, token: &SpannedToken) -> bool {
        if ALLOWED_STATE_SELECTORS.contains(name) {
            return true;
        }

        self.ast_errors.push(
            TypeError::InvalidSelector {
                msg: Some(&format!("No state named \"{}\" exists.", name)),
            },
            self.range_from_span(token.span()),
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
    Err(&'a Node<'a>),
}
