use crate::{node_token_matches, token_kind_list};
use crate::lexer::{SpannedToken, Token, TokenKind, TOKEN_KIND_CONSTRUCT_DELIMITERS, TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS};
use crate::list::{Stringified, TokenKindList};
use crate::parser::parse_error::{ParseError, ParseErrorMessage};
use crate::parser::types::*;
use crate::parser::Parser;

impl<'a> Parser<'a> {
    /// Many declarations in rsml just have a datatype after them.
    /// So we can use the same function to parse them.
    pub(crate) fn parse_declaration_with_datatype(
        &mut self,
        node: Node<'a>,
        declaration_token_kind: TokenKind,
        constructor: fn(declaration: Node<'a>, body: Option<Box<Construct<'a>>>, terminator: Option<Node<'a>>) -> Construct<'a>
    ) -> Parsed<'a> {
        if node.token.value().kind() != declaration_token_kind { return Parsed (Some(node), None) }
        let declaration_node = node;

        let node = self.advance_without_flags();
        self.did_advance = true;

        let (node_status, body_nodes) =
            self.parse_datatype(node, TOKEN_KIND_CONSTRUCT_DELIMITERS);
        let body_nodes = body_nodes.map(|x| Box::new(x));

        let terminator = match node_status {
            NodeStatus::Exists => match self.advance_until(token_kind_list![ SemiColon ], &TOKEN_KIND_CONSTRUCT_DELIMITERS) {
                Some(Ok(node)) => node,
                Some(Err(node)) => return Parsed (Some(node), Some(constructor(
                    declaration_node, body_nodes, None
                ))),
                None => return Parsed (None, Some(constructor(
                    declaration_node, body_nodes, None
                ))),
            },

            NodeStatus::Err(node) => {
                if node_token_matches!(node, SemiColon) {
                    node

                } else {
                    let construct = constructor(
                        declaration_node, body_nodes, None
                    );

                    self.ast_errors.push(
                        ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::SemiColon.name())) },
                        self.range_from_span(clamp_span_to_end(construct.end()))
                    );

                    return Parsed (Some(node), Some(construct))
                }
            },

            NodeStatus::None => {
                let construct = constructor(
                    declaration_node, body_nodes, None
                );

                self.ast_errors.push(
                    ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::SemiColon.name())) },
                    self.range_from_span(clamp_span_to_end(construct.end()))
                );

                return Parsed (None, Some(construct))
            }
        };

        Parsed (self.advance(), Some(constructor(
            declaration_node, body_nodes, Some(terminator)
        )))
    }

    pub(crate) fn parse_derive(
        &mut self, node: Node<'a>
    ) -> Parsed<'a> {
        self.parse_declaration_with_datatype(
            node,
            TokenKind::DeriveDeclaration,
            |declaration, body, terminator| {
                Construct::Derive { declaration, body, terminator }
            }
        )
    }

    pub(crate) fn parse_priority(
        &mut self, node: Node<'a>
    ) -> Parsed<'a> {
        self.parse_declaration_with_datatype(
            node,
            TokenKind::PriorityDeclaration,
            |declaration, body, terminator| {
                Construct::Priority { declaration, body, terminator }
            }
        )
    }

    pub(crate) fn parse_name(
        &mut self, node: Node<'a>
    ) -> Parsed<'a> {
        self.parse_declaration_with_datatype(
            node,
            TokenKind::NameDeclaration,
            |declaration, body, terminator| {
                Construct::Name { declaration, body, terminator }
            }
        )
    }

    pub(crate) fn parse_tween(
        &mut self, node: Node<'a>
    ) -> Parsed<'a> {
        if !node_token_matches!(node, TweenDeclaration) { return Parsed (Some(node), None) }

        let declaration_node = node;

        let name_node = match self.advance_until(
            token_kind_list!("tween name", [ Identifier ]),
            &TOKEN_KIND_CONSTRUCT_DELIMITERS
        ) {
            Some(Ok(node)) => Some(node),
            Some(Err(node)) => return Parsed (Some(node), Some(
                Construct::Tween { declaration: declaration_node, name: None, body: None, terminator: None }
            )),
            None => return Parsed (None, Some(
                Construct::Tween { declaration: declaration_node, name: None, body: None, terminator: None }
            )),
        };

        let node = self.advance_without_flags();
        self.did_advance = true;

        let (node_status, body_nodes) =
            self.parse_datatype(node, TOKEN_KIND_CONSTRUCT_DELIMITERS);
        let body_nodes = body_nodes.map(|x| Box::new(x));

        let terminator = match node_status {
            NodeStatus::Exists => match self.advance_until(token_kind_list![ SemiColon ], &TOKEN_KIND_CONSTRUCT_DELIMITERS) {
                Some(Ok(node)) => node,
                Some(Err(node)) => return Parsed (Some(node), Some(
                    Construct::Tween { declaration: declaration_node, name: name_node, body: body_nodes, terminator: None }
                )),
                None => return Parsed (None, Some(
                    Construct::Tween { declaration: declaration_node, name: name_node, body: body_nodes, terminator: None }
                )),
            },

            NodeStatus::Err(node) => {
                if node_token_matches!(node, SemiColon) {
                    node

                } else {
                    let construct = Construct::Tween {
                        declaration: declaration_node, name: name_node, body: body_nodes, terminator: None
                    };

                    self.ast_errors.push(
                        ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::SemiColon.name())) },
                        self.range_from_span(clamp_span_to_end(construct.end()))
                    );

                    return Parsed (Some(node), Some(construct))
                }
            },

            NodeStatus::None => {
                let construct = Construct::Tween {
                    declaration: declaration_node, name: name_node, body: body_nodes, terminator: None
                };

                self.ast_errors.push(
                    ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::SemiColon.name())) },
                    self.range_from_span(clamp_span_to_end(construct.end()))
                );

                return Parsed (None, Some(construct))
            }
        };

        Parsed (self.advance(), Some(Construct::Tween {
            declaration: declaration_node, name: name_node, body: body_nodes, terminator: Some(terminator)
        }))
    }

    // TODO: properly implement macros.
    pub(crate) fn parse_macro_call(
        &mut self, node: Node<'a>
    ) -> Parsed<'a> {
        if !node_token_matches!(node, MacroCallIdentifier(_)) { return Parsed (Some(node), None) }

        self.parse_macro_call_body(node)
    }

    pub(crate) fn parse_macro_call_body(&mut self, name_node: Node<'a>) -> Parsed<'a> {
        let open_node = match self.advance_until(token_kind_list![ ParensOpen ], &TOKEN_KIND_CONSTRUCT_DELIMITERS) {
            Some(Ok(node)) => node,
            Some(Err(node)) => return Parsed (Some(node), Some(Construct::MacroCall { name: name_node, body: None, terminator: None })),
            None => return Parsed (None, Some(Construct::MacroCall { name: name_node, body: None, terminator: None })),
        };

        let mut body_content: Vec<Construct<'a>> = vec![];

        let mut parens_nestedness: usize = 0;

        let close_node = loop {
            let node = self.advance();

            match node {
                Some(Node { token: SpannedToken(_, Token::ParensOpen, _), .. }) =>
                    parens_nestedness += 1,

                Some(node @ Node { token: SpannedToken(_, Token::ParensClose, _), .. }) => {
                    if parens_nestedness == 0 { break node }
                    else { parens_nestedness -= 1 }
                },

                Some(node) => body_content.push(Construct::Node { node }),

                None => {
                    let construct = Construct::MacroCall {
                        name: name_node,
                        body: Some(Delimited::new(open_node, Some(body_content), None)),
                        terminator: None
                    };

                    self.ast_errors.push(
                        ParseError::MissingToken {
                            msg: Some(ParseErrorMessage::Expected(TokenKind::ParensClose.name()))
                        },
                        self.range_from_span(construct.span())
                    );

                    return Parsed (None, Some(construct))
                }
            }
        };

        let terminator_node = match self.advance_until(token_kind_list![SemiColon], &TOKEN_KIND_CONSTRUCT_DELIMITERS) {
            Some(Ok(node)) => node,
            Some(Err(node)) => return Parsed (Some(node), Some(Construct::MacroCall {
                name: name_node,
                body: Some(Delimited::new(open_node, Some(body_content), Some(close_node))),
                terminator: None
            })),
            None => return Parsed (None, Some(Construct::MacroCall {
                name: name_node,
                body: Some(Delimited::new(open_node, Some(body_content), Some(close_node))),
                terminator: None
            })),
        };

        Parsed (self.advance(), Some(Construct::MacroCall {
            name: name_node,
            body: Some(Delimited::new(open_node, Some(body_content), Some(close_node))),
            terminator: Some(terminator_node)
        }))
    }

    pub(crate) fn parse_macro(&mut self, node: Node<'a>) -> Parsed<'a> {
        if !node_token_matches!(node, MacroDeclaration) { return Parsed (Some(node), None) }

        let declaration_node = node;

        let name_node = match self.advance_until(
            token_kind_list!("macro name", [ Identifier ]),
            &TOKEN_KIND_CONSTRUCT_DELIMITERS
        ) {
            Some(Ok(node)) => Some(node),
            Some(Err(node)) => {
                let construct = Construct::Macro { declaration: declaration_node, name: None, args: None, body: None };
                return Parsed (Some(node), Some(construct))
            },
            None => {
                let construct = Construct::Macro { declaration: declaration_node, name: None, args: None, body: None };
                return Parsed (self.advance(), Some(construct))
            },
        };

        let args_or_body_node = match self.advance_until(
            token_kind_list!("macro arguments or body", [ ScopeOpen, ParensOpen ]),
            &TOKEN_KIND_CONSTRUCT_DELIMITERS
        ) {
            Some(Ok(node)) => node,
            Some(Err(node)) => return Parsed (Some(node), Some(
                Construct::Macro { declaration: declaration_node, name: name_node, args: None, body: None }
            )),
            None => return Parsed (None, Some(
                Construct::Macro { declaration: declaration_node, name: name_node, args: None, body: None }
            )),
        };

        if matches!(args_or_body_node.token.value(), Token::ParensOpen) {
            self.parse_macro_args(args_or_body_node, declaration_node, name_node)

        } else {
            self.parse_macro_body(args_or_body_node, declaration_node, name_node, None)
        }
    }

    fn parse_macro_args(
        &mut self,
        args_open_node: Node<'a>,
        declaration_node: Node<'a>,
        name_node: Option<Node<'a>>
    ) -> Parsed<'a> {
        let mut node = match self.advance_until(token_kind_list![
            MacroArgIdentifier, Comma, ParensClose
        ], &TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS) {
            Some(Ok(node)) => node,
            Some(Err(node)) => return Parsed (Some(node), Some(
                Construct::Macro {
                    declaration: declaration_node,
                    name: name_node,
                    args: Some(Delimited { left: args_open_node, content: None, right: None }),
                    body: None
                }
            )),
            None => return Parsed (None, Some(
                Construct::Macro {
                    declaration: declaration_node,
                    name: name_node,
                    args: Some(Delimited { left: args_open_node, content: None, right: None }),
                    body: None
                }
            )),
        };

        let mut last_token_value = node.token.value().clone();
        let mut last_token_span = node.token.span();

        if matches!(last_token_value, Token::ParensClose) {
            return self.parse_macro_body_open(
                declaration_node, name_node, args_open_node, None, Some(node)
            );
        }

        let mut args = vec![Construct::Node { node }];

        loop {
            let advance_until_result = match last_token_value {
                Token::Comma => self.advance_until(token_kind_list![
                    MacroArgIdentifier, ParensClose
                ], &TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS),

                _ => self.advance_until(token_kind_list![
                    MacroArgIdentifier, Comma, ParensClose
                ], &TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS)
            };

            node = match advance_until_result {
                Some(Ok(node)) => node,
                Some(Err(node)) => return Parsed (Some(node), Some(
                    Construct::Macro {
                        declaration: declaration_node,
                        name: name_node,
                        args: Some(Delimited::new(args_open_node, Some(args), None)),
                        body: None
                    }
                )),
                None => return Parsed (None, Some(
                    Construct::Macro {
                        declaration: declaration_node,
                        name: name_node,
                        args: Some(Delimited::new(args_open_node, Some(args), None)),
                        body: None
                    }
                )),
            };

            let token_span = node.token.span();
            let token_value = node.token.value().clone();

            if matches!(token_value, Token::ParensClose) {
                return self.parse_macro_body_open(
                    declaration_node, name_node, args_open_node, Some(args), Some(node)
                );
            };

            args.push(Construct::Node { node });

            if matches!((&last_token_value, &token_value), (Token::MacroArgIdentifier(_), Token::MacroArgIdentifier(_))) {
                self.ast_errors.push(
                    ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::Comma.name())) },
                    self.range_from_span((last_token_span.1 - 1, last_token_span.1))
                );
            }

            last_token_value = token_value;
            last_token_span = token_span;
        }
    }

    fn parse_macro_body_open(
        &mut self,
        declaration_node: Node<'a>,
        name_node: Option<Node<'a>>,
        args_open_node: Node<'a>,
        args_content_node: Option<Vec<Construct<'a>>>,
        args_close_node: Option<Node<'a>>,
    ) -> Parsed<'a> {
        let body_node = match self.advance_until(token_kind_list![ ScopeOpen ], &TOKEN_KIND_CONSTRUCT_DELIMITERS) {
            Some(Ok(node)) => node,
            Some(Err(node)) => return Parsed (Some(node), Some(
                Construct::Macro {
                    declaration: declaration_node,
                    name: name_node,
                    args: Some(Delimited { left: args_open_node, content: args_content_node, right: args_close_node }),
                    body: None
                }
            )),
            None => return Parsed (None, Some(
                Construct::Macro {
                    declaration: declaration_node,
                    name: name_node,
                    args: Some(Delimited { left: args_open_node, content: args_content_node, right: args_close_node }),
                    body: None
                }
            )),
        };

        return self.parse_macro_body(
            body_node,
            declaration_node,
            name_node,
            Some(Delimited::new(args_open_node, args_content_node, args_close_node))
        )
    }

    pub(crate) fn parse_macro_body(
        &mut self,
        body_open_node: Node<'a>,
        declaration_node: Node<'a>,
        name_node: Option<Node<'a>>,
        args_node: Option<Delimited<'a>>
    ) -> Parsed<'a> {
        let Some(node) = self.advance() else {
            self.ast_errors.push(
                ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ScopeClose.name())) },
                self.range_from_span(clamp_span_to_end(body_open_node.token.end()))
            );
            return Parsed (None, Some(Construct::Macro {
                declaration: declaration_node,
                name: name_node,
                args: args_node,
                body: Some(Delimited::new(body_open_node, None, None))
            }));
        };

        if node_token_matches!(node, ScopeClose) {
            return Parsed (self.advance(), Some(Construct::Macro {
                declaration: declaration_node,
                name: name_node,
                args: args_node,
                body: Some(Delimited::new(body_open_node, None, Some(node)))
            }))
        }

        let mut body_content: Vec<Construct<'a>> = vec![];

        let (node, parse_ended_reason) =
            self.parse_loop_inner(node,|parser, mut node| {
                node = parser.parse_macro(node).handle_construct_with_err(
                    &mut body_content, &mut parser.ast_errors, &parser.lexer.rope, Some("other macros")
                )?;

                node = parser.parse_macro_call(node).handle_construct(&mut body_content)?;

                node = parser.parse_derive(node).handle_construct_with_err(
                    &mut body_content, &mut parser.ast_errors, &parser.lexer.rope, Some("non-global scopes")
                )?;

                node = parser.parse_priority(node).handle_construct(&mut body_content)?;
                node = parser.parse_name(node).handle_construct(&mut body_content)?;

                node = parser.parse_tween(node).handle_construct(&mut body_content)?;

                node = parser.parse_static_token_assignment(node).handle_construct(&mut body_content)?;
                node = parser.parse_token_assignment(node).handle_construct(&mut body_content)?;

                node = parser.parse_property_assignment_or_rule_scope(node).handle_construct(&mut body_content)?;
                node = parser.parse_rule_scope_selector_begin(node).handle_construct(&mut body_content)?;

                node = parser.parse_invalid_declaration(node)?;
                node = parser.parse_none(node).handle_construct(&mut body_content)?;

                let end_parsing = node_token_matches!(node, ScopeClose);
                Some((node, end_parsing))
            });

        if matches!(parse_ended_reason, ParseEndedReason::Manual) {
            return Parsed (self.advance(), Some(Construct::Macro {
                declaration: declaration_node,
                name: name_node,
                args: args_node,
                body: Some(Delimited::new(body_open_node, Some(body_content), node))
            }))

        } else {
            let construct = Construct::Macro {
                declaration: declaration_node,
                name: name_node,
                args: args_node,
                body: Some(Delimited::new(body_open_node, Some(body_content), None))
            };

            self.ast_errors.push(
                ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ScopeClose.name())) },
                self.range_from_span(clamp_span_to_end(construct.end()))
            );

            Parsed (self.advance(), Some(construct))
        }
    }
}
