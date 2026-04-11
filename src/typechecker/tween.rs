use crate::{
    lexer::{SpannedToken, Token},
    parser::{AstErrors, Construct, Delimited, Node},
};

use super::{PushTypeError, Typechecker, type_error::*};

fn is_number(construct: &Construct) -> bool {
    matches!(
        construct,
        Construct::Node {
            node: Node {
                token: SpannedToken(_, Token::Number(_), _),
                ..
            },
        }
    )
}

fn is_enum(construct: &Construct, expected_name: &str) -> bool {
    match construct {
        Construct::Enum {
            name:
                Some(Node {
                    token:
                        SpannedToken(
                            _,
                            Token::StateSelectorOrEnumPart(Some(name))
                            | Token::TagSelectorOrEnumPart(Some(name)),
                            _,
                        ),
                    ..
                }),
            ..
        } => *name == expected_name,

        // Enum shorthand like `:InOut`
        Construct::Node {
            node: Node {
                token: SpannedToken(_, Token::StateSelectorOrEnumPart(Some(_)), _),
                ..
            },
        } => true,

        _ => false,
    }
}

fn is_comma(construct: &Construct) -> bool {
    matches!(
        construct,
        Construct::Node {
            node: Node {
                token: SpannedToken(_, Token::Comma, _),
                ..
            },
        }
    )
}

impl<'a> Typechecker<'a> {
    pub(super) fn typecheck_tween(
        &self,
        body: &Construct<'a>,
        ast_errors: &mut AstErrors,
    ) {
        match body {
            // Case 1: bare number — `@tween Prop .5;`
            construct if is_number(construct) => (),

            // Case 2: tuple — `@tween Prop (.5, :InOut, :In);`
            Construct::Table {
                body: Delimited { content: Some(items), .. },
            } => {
                let args: Vec<&Construct<'a>> = items.iter().filter(|item| !is_comma(item)).collect();

                if args.is_empty() {
                    ast_errors.push(
                        TypeError::InvalidType { expected: Some(Datatype::Tween) },
                        self.parsed.range_from_span(body.span()),
                    );
                    return;
                }

                // Arg 0: must be a number
                if !is_number(args[0]) {
                    ast_errors.push(
                        TypeError::InvalidTweenArg { expected: "number" },
                        self.parsed.range_from_span(args[0].span()),
                    );
                }

                // Arg 1: optional, must be Enum.EasingStyle
                if let Some(arg) = args.get(1) {
                    if !is_enum(arg, "EasingStyle") {
                        ast_errors.push(
                            TypeError::InvalidTweenArg { expected: "Enum.EasingStyle" },
                            self.parsed.range_from_span(arg.span()),
                        );
                    }
                }

                // Arg 2: optional, must be Enum.EasingDirection
                if let Some(arg) = args.get(2) {
                    if !is_enum(arg, "EasingDirection") {
                        ast_errors.push(
                            TypeError::InvalidTweenArg { expected: "Enum.EasingDirection" },
                            self.parsed.range_from_span(arg.span()),
                        );
                    }
                }

                // Too many args
                for arg in args.iter().skip(3) {
                    ast_errors.push(
                        TypeError::InvalidType { expected: Some(Datatype::Tween) },
                        self.parsed.range_from_span(arg.span()),
                    );
                }
            }

            // Anything else is invalid
            _ => {
                ast_errors.push(
                    TypeError::InvalidType { expected: Some(Datatype::Tween) },
                    self.parsed.range_from_span(body.span()),
                );
            }
        }
    }
}
