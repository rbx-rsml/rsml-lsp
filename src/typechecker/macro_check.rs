use std::collections::HashSet;

use crate::{
    lexer::Token,
    parser::{AstErrors, Construct, Delimited, MacroBody, MacroBodyContent},
    range_from_span::RangeFromSpan,
};

use super::{PushTypeError, Typechecker, type_error::*};

impl<'a> Typechecker<'a> {
    pub(super) fn typecheck_macro(
        &self,
        args: &Option<Delimited<'a>>,
        body: &Option<MacroBody<'a>>,
        ast_errors: &mut AstErrors,
    ) {
        let macro_args = collect_macro_arg_names(args);
        let Some(body) = body else { return };

        match &body.content {
            MacroBodyContent::Construct(Some(content)) => {
                self.typecheck_macro_body_content(content, &macro_args, ast_errors);
            }
            MacroBodyContent::Assignment(Some(content)) => {
                self.validate_macro_arg_refs(content, Some(&macro_args), ast_errors);
            }
            MacroBodyContent::Selector(_) => {}
            _ => {}
        }
    }

    fn typecheck_macro_body_content(
        &self,
        content: &Vec<Construct<'a>>,
        macro_args: &HashSet<&str>,
        ast_errors: &mut AstErrors,
    ) {
        for construct in content {
            match construct {
                Construct::Assignment { right, .. } => {
                    if let Some(right) = right {
                        self.validate_macro_arg_refs(right, Some(macro_args), ast_errors);
                    }
                }

                Construct::Rule { body, .. } => {
                    if let Some(body) = body {
                        if let Some(content) = &body.content {
                            self.typecheck_macro_body_content(content, macro_args, ast_errors);
                        }
                    }
                }

                Construct::Tween { body, .. } => {
                    if let Some(body) = body {
                        self.validate_macro_arg_refs(body, Some(macro_args), ast_errors);
                    }
                }

                _ => ()
            }
        }
    }

    pub(super) fn validate_macro_arg_refs(
        &self,
        construct: &Construct<'a>,
        macro_args: Option<&HashSet<&str>>,
        ast_errors: &mut AstErrors,
    ) {
        match construct {
            Construct::Node { node } => {
                if let Token::MacroArgIdentifier(name) = node.token.value() {
                    let is_valid = match macro_args {
                        Some(args) => name.is_some_and(|arg_name| args.contains(arg_name)),
                        None => false,
                    };

                    if !is_valid {
                        if let Some(arg_name) = name {
                            ast_errors.push(
                                TypeError::InvalidMacroArg {
                                    msg: &format!("No macro argument named \"{}\" exists.", arg_name)
                                },
                                self.range_from_span(node.token.span()),
                            );
                        } else {
                            ast_errors.push(
                                TypeError::InvalidMacroArg {
                                    msg: "Missing macro argument name."
                                },
                                self.range_from_span(node.token.span()),
                            );
                        }
                    }
                }
            }

            Construct::MathOperation { left, right, .. } => {
                self.validate_macro_arg_refs(left, macro_args, ast_errors);
                if let Some(right) = right {
                    self.validate_macro_arg_refs(right, macro_args, ast_errors);
                }
            }

            Construct::Table { body } => {
                if let Some(content) = &body.content {
                    for item in content {
                        self.validate_macro_arg_refs(item, macro_args, ast_errors);
                    }
                }
            }

            Construct::AnnotatedTable { body, .. } => {
                if let Some(body) = body {
                    if let Some(content) = &body.content {
                        for item in content {
                            self.validate_macro_arg_refs(item, macro_args, ast_errors);
                        }
                    }
                }
            }

            _ => ()
        }
    }

    fn range_from_span(&self, span: (usize, usize)) -> tower_lsp::lsp_types::Range {
        tower_lsp::lsp_types::Range::from_span(&self.parsed.lexer.rope, span)
    }
}

fn collect_macro_arg_names<'a>(args: &Option<Delimited<'a>>) -> HashSet<&'a str> {
    let mut names = HashSet::new();
    if let Some(args) = args {
        if let Some(content) = &args.content {
            for construct in content {
                if let Construct::Node { node } = construct {
                    if let Token::MacroArgIdentifier(Some(name)) = node.token.value() {
                        names.insert(*name);
                    }
                }
            }
        }
    }
    names
}
