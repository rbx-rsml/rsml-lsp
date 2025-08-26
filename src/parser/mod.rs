use std::{collections::HashSet, mem::discriminant, sync::LazyLock};

use ropey::Rope;
use tower_lsp::lsp_types::{Diagnostic, NumberOrString, Range};

use crate::{guarded_unwrap, guarded_unwrap_advance, lexer::{Lexer, MultilineString, SpannedToken, Token, TokenKind, DECLARATION_NAMES, TOKEN_KIND_ADD_SUB_PRECEDENCE, TOKEN_KIND_CONSTRUCT_DELIMITERS, TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS, TOKEN_KIND_OPERATOR_PRECEDENCE}, list::{Stringified, TokenKindList}, parser::parse_error::ParseErrorMessage, range_from_span::RangeFromSpan};

mod parse_error;
use parse_error::ParseError;

type SymResult<T> = Result<T, T>;

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    last_token_end: usize,

    pub ast: Vec<Construct<'a>>,
    pub ast_errors: AstErrors,

    pub did_advance: bool
}

type Trivia<'a> = Vec<SpannedToken<'a>>;

#[derive(Debug)]
pub struct Node<'a> {
    pub token: SpannedToken<'a>,
    pub leading_trivia: Option<Trivia<'a>>
}

trait UpdateLastTokenEnd {
    fn update_last_token_end(self, parser: &mut Parser) -> Self;
}

impl<'a> UpdateLastTokenEnd for Option<Node<'a>> {
    fn update_last_token_end(self, parser: &mut Parser) -> Self {
        // If there is a valid token then we update the
        // end position of the most recent (this) token.
        if let Some(Node { token: SpannedToken (_, _, end), .. }) = self {
            parser.last_token_end = end
        };

        self
    }
}

trait ToStatus<'a> {
    fn to_status(self) -> NodeStatus<'a>;
}

impl<'a> ToStatus<'a> for Option<Node<'a>> {
    fn to_status(self) -> NodeStatus<'a> {
        match self {
            Some(node) => NodeStatus::Err(node),
            None => NodeStatus::None
        }
    }
}

impl<'a> ToStatus<'a> for Node<'a> {
    fn to_status(self) -> NodeStatus<'a> {
        NodeStatus::Err(self)
    }
}

struct Parsed<'a, T = Construct<'a>> (Option<Node<'a>>, Option<T>);

impl<'a> Parsed<'a> {
    #[inline(always)]
    fn none() -> Self {
        Self (None, None)
    }

    fn handle_construct(
        self,
        ast: &mut Vec<Construct<'a>>
    ) -> Option<Node<'a>> {
        if let Some(construct) = self.1 { ast.push(construct) };
        return self.0
    }

    fn handle_construct_with_err(
        self,
        ast: &mut Vec<Construct<'a>>,
        ast_errors: &mut AstErrors,
        rope: &Rope,
        context: Option<&str>
    ) -> Option<Node<'a>> {
        if let Some(construct) = self.1 {
            ast_errors.push(
                ParseError::UnexpectedTokens {
                    msg: Some(ParseErrorMessage::NotAllowed { name: construct.name_plural(), context })
                },
                Range::from_span(rope, construct.span())
            );

            ast.push(construct)
        };

        return self.0
    }

    fn handle_construct_with_err_if<F: FnMut(&Construct<'a>) -> bool>(
        self,
        ast: &mut Vec<Construct<'a>>,
        ast_errors: &mut AstErrors,
        rope: &Rope,
        context: Option<&str>,
        mut callback: F
    ) -> Option<Node<'a>> {
        if let Some(construct) = self.1 {
            if callback(&construct) {
                ast_errors.push(
                    ParseError::UnexpectedTokens {
                        msg: Some(ParseErrorMessage::NotAllowed { name: construct.name_plural(), context })
                    },
                    Range::from_span(rope, construct.span())
                );
            }

            ast.push(construct)
        };

        return self.0
    }
}

macro_rules! token_kind_list {
    ($str:literal, [ $( $name:ident ),* ]) => {
        &TokenKindList::new_with_stringified([$(
            (TokenKind::$name, discriminant(&TokenKind::$name))
        ),*], Stringified::Single(String::from($str)))
    };

    ($( $name:ident ),*) => {
        &TokenKindList::new([$(
            (TokenKind::$name, discriminant(&TokenKind::$name))
        ),*])
    };

    ([ $( $name:ident ),* ]) => {
        token_kind_list!($( $name ),*)
    };
}

#[macro_export]
macro_rules! node_token_matches {
    ($node:ident, Some($( $name:ident )|*)) => {
        matches!($node, Some(Node { token: SpannedToken (_, $( Token::$name )|*, _), .. }))
    };

    ($node:ident, $( $name:ident )|*) => {
        matches!($node, Node { token: SpannedToken (_, $( Token::$name )|*, _), .. })
    };

    ($node:ident, Some($( $name:ident($( $args:pat ),*) )|*)) => {
        matches!($node, Some(Node { token: SpannedToken(_, $( Token::$name($( $args ),*) )|*, _), .. }))
    };

    ($node:ident, $( $name:ident($( $args:pat ),*) )|*) => {
        matches!($node, Node { token: SpannedToken(_, $( Token::$name($( $args ),*) )|*, _), .. })
    };
}

impl<'a> Parser<'a> {
    pub fn new(lexer: Lexer<'a>) -> Self {
        let mut parser = Self {
            lexer,
            last_token_end: 0,

            ast: Vec::new(),
            ast_errors: AstErrors::new(),

            did_advance: false
        };

        parser.parse_loop(|parser, mut node| {
            node = parser.parse_macro(node).handle_construct(&mut parser.ast)?;
            node = parser.parse_macro_call(node).handle_construct(&mut parser.ast)?;

            node = parser.parse_derive(node).handle_construct(&mut parser.ast)?;

            node = parser.parse_priority(node).handle_construct_with_err(
                &mut parser.ast, &mut parser.ast_errors, &parser.lexer.rope, Some("the global scope")
            )?;

            node = parser.parse_name(node).handle_construct_with_err(
                &mut parser.ast, &mut parser.ast_errors, &parser.lexer.rope, Some("the global scope")
            )?;

            node = parser.parse_static_token_assignment(node).handle_construct(&mut parser.ast)?;

            node = parser.parse_token_assignment(node).handle_construct(&mut parser.ast)?;

            node = parser.parse_property_assignment_or_rule_scope(node).handle_construct_with_err_if(
                &mut parser.ast, &mut parser.ast_errors, &parser.lexer.rope, Some("the global scope"),
                |x| matches!(x, Construct::Assignment { .. })
            )?;
            node = parser.parse_rule_scope_selector_begin(node).handle_construct(&mut parser.ast)?;

            node = parser.parse_invalid_declaration(node)?;

            node = parser.parse_none(node).handle_construct(&mut parser.ast)?;

            Some(node)
        });

        parser
    }

    pub fn range_from_span(&self, span: (usize, usize)) -> Range {
        Range::from_span(&self.lexer.rope, span)
    }

    fn next_token(&mut self) -> Option<SpannedToken<'a>> {
        self.lexer.next()
    }

    fn token_slice(&self) -> &'a str {
        self.lexer.slice()
    }

    fn handle_multiline_string_error(
        &mut self,
        token: &SpannedToken,
        expected_nestedness: usize
    ) {
        self.ast_errors.push(
            ParseError::MissingToken {
                msg: Some(ParseErrorMessage::Expected(&format!("\"]{}]\"", "=".repeat(expected_nestedness))))
            },
            self.range_from_span(clamp_span_to_end(token.end()))
        )
    }

    fn next_node(&mut self) -> Option<Node<'a>> {
        let mut token = self.next_token()?;

        match token.value() {
            Token::CommentMulti(MultilineString { nestedness: Err(expected_nestedness), .. }) => {
                self.handle_multiline_string_error(&token, *expected_nestedness)
            },

            Token::CommentSingle(_) | Token::CommentMulti(MultilineString { nestedness: Ok(_), .. }) => (),

            _ => return Some(Node {
                token: token,
                leading_trivia: None
            })
        }

        let mut leading_trivia = vec![ token ];

        loop {
            token = guarded_unwrap!(
                self.next_token(),
                return Some(Node {
                    token: SpannedToken::new(self.last_token_end, Token::None, self.last_token_end),
                    leading_trivia: Some(leading_trivia)
                })
            );

            match token.value() {
                Token::CommentMulti(MultilineString { nestedness: Err(expected_nestedness), .. }) => {
                    self.handle_multiline_string_error(&token, *expected_nestedness);

                    leading_trivia.push(token);
                },

                Token::CommentSingle(_) | Token::CommentMulti(MultilineString { nestedness: Ok(_), .. }) =>
                    leading_trivia.push(token),

                _ => return Some(Node {
                    token: token,
                    leading_trivia: Some(leading_trivia)
                })
            }
        }
    }

    /// Advances to the next valid node - doesn't update the `did_advance` or `last_token_end` flags.
    fn advance_without_flags<'b>(
        &mut self
    ) -> Option<Node<'a>> {
        match self.next_node()? {
            Node { token: SpannedToken (span_start, Token::Error, mut span_end), .. } => loop {
                match self.next_node() {
                    Some(Node { token: SpannedToken (_, Token::Error, next_span_end), .. }) => span_end = next_span_end,

                    node => {
                        // Pushes an error for all of the previous error nodes.
                        self.ast_errors.push(
                            ParseError::UnexpectedTokens { msg: None },
                            self.range_from_span((span_start, span_end))
                        );

                        break node 
                    }
                }
            },

            node => Some(node)
        }
    }
    
    /// Advances to the next valid node.
    fn advance(&mut self) -> Option<Node<'a>> {
        let node = self.advance_without_flags()
            .update_last_token_end(self);
        self.did_advance = true;

        node
    }

    fn advance_until_core_loop<const N: usize>(
        &mut self,
        allow_list: &TokenKindList<N>,
        construct_delimiters: &LazyLock<HashSet<TokenKind>>,
        span_start: usize, mut span_end: usize
    ) -> Option<SymResult<Node<'a>>> {
        loop {
            match self.next_node() {
                Some(Node { token: SpannedToken (_, Token::Error, next_span_end), .. }) => span_end = next_span_end,

                Some(node) => {
                    let token = &node.token;
                    let token_kind = &token.value().kind();

                    if allow_list.has_discriminant(&discriminant(token_kind)) {
                        // We have found the valid end token!

                        // Pushes an error for all of the previous error nodes.
                        // We don't need to specify the expected tokens in the error
                        // message as we have found the expected token.
                        self.ast_errors.push(
                            ParseError::UnexpectedTokens { msg: None },
                            self.range_from_span((span_start, span_end))
                        );

                        self.last_token_end = token.end();

                        break Some(Ok(node))

                    } else if construct_delimiters.contains(token_kind) {
                        // Pushes an error for all of the previous error nodes.
                        self.ast_errors.push(
                            ParseError::UnexpectedTokens {
                                msg: allow_list.to_string().as_deref().map(|x| ParseErrorMessage::Expected(x))
                            },
                            self.range_from_span((span_start, span_end))
                        );
                        
                        break Some(Err(node))

                    } else {
                        // Token was invalid so we need to adjust the span_end to accomodate it.
                        span_end = token.end()
                    }
                },

                None => {
                    // Pushes an error for all of the previous error nodes.
                    self.ast_errors.push(
                        ParseError::UnexpectedTokens {
                            msg: allow_list.to_string().as_deref().map(|x| ParseErrorMessage::Expected(x))
                        },
                        self.range_from_span((span_start, span_end))
                    );

                    break None
                }
            }
        }
    }
    
    /// Advances to the next valid node which has a token in the allow list - does not set the `did_advance` flag.
    fn advance_until_without_flag<const N: usize>(
        &mut self,
        allow_list: &TokenKindList<N>,
        construct_delimiters: &LazyLock<HashSet<TokenKind>>
    ) -> Option<SymResult<Node<'a>>> {
        match self.next_node() {
            Some(Node { token: SpannedToken (span_start, Token::Error, span_end), .. }) => {
                self.advance_until_core_loop(allow_list, construct_delimiters, span_start, span_end)
            },

            Some(node) => {
                let token = &node.token;
                let token_kind = &token.value().kind();

                if allow_list.has_discriminant(&discriminant(&token_kind)) {
                    // We have found the valid end token!

                    self.last_token_end = token.end();

                    Some(Ok(node))

                } else if construct_delimiters.contains(token_kind) {
                    self.ast_errors.push(
                        ParseError::MissingToken { 
                            msg: allow_list.to_string().as_deref().map(|x| ParseErrorMessage::Expected(x))
                        },
                        self.range_from_span(clamp_span_to_end(self.last_token_end))
                    );
                    
                    Some(Err(node))

                } else {
                    self.advance_until_core_loop(allow_list, construct_delimiters, token.start(), token.end())
                }
            },

            // Push a missing token error as we reached the
            // end of the file without finding an expected token.
            None => {
                // Pushes an error for all of the previous error nodes.
                self.ast_errors.push(
                    ParseError::MissingToken {
                        msg: allow_list.to_string().as_deref().map(|x| ParseErrorMessage::Expected(x))
                    },
                    self.range_from_span(clamp_span_to_end(self.last_token_end))
                );
                
                None
            }
        }
    }

    /// Advances to the next valid node which has a token in the allow list.
    fn advance_until<const N: usize>(
        &mut self,
        allow_list: &TokenKindList<N>,
        construct_delimiters: &LazyLock<HashSet<TokenKind>>
    ) -> Option<SymResult<Node<'a>>> {
        let next = self.advance_until_without_flag(allow_list, construct_delimiters);
        self.did_advance = true;
        next
    }

    fn node_is_kind_else_advance_until<const N: usize>(
        &mut self,
        node: Node<'a>,
        allow_list: &TokenKindList<N>,
        construct_delimiters: &LazyLock<HashSet<TokenKind>>
    ) -> Option<SymResult<Node<'a>>> {
        if allow_list.has_discriminant(&node.token.value().discriminant()) { return Some(Ok(node)) };

        if construct_delimiters.contains(&node.token.value().kind()) {
            // Pushes an error for the previous error node.
            self.ast_errors.push(
                ParseError::MissingToken {
                    msg: allow_list.to_string().as_deref().map(|x| ParseErrorMessage::Expected(x))
                },
                self.range_from_span(clamp_span_to_end(self.last_token_end))
            );
            
            return Some(Err(node))
        }

        let last_token = node.token;

        match self.next_node() {
            Some(Node { token: SpannedToken (_, Token::Error, span_end), .. }) => {
                self.advance_until_core_loop(allow_list, construct_delimiters, last_token.start(), span_end)
            },

            Some(node) => {
                let token = &node.token;
                let token_kind = &token.value().kind();

                if allow_list.has_discriminant(&discriminant(&token_kind)) {
                    // We have found the valid end token!

                    // Pushes an error for the previous error node.
                    self.ast_errors.push(
                        ParseError::UnexpectedTokens { msg: None },
                        self.range_from_span(last_token.span())
                    );

                    self.last_token_end = token.end();

                    Some(Ok(node))

                } else if construct_delimiters.contains(token_kind) {
                    // Pushes an error for the previous error node.
                    self.ast_errors.push(
                        ParseError::UnexpectedTokens {
                            msg: allow_list.to_string().as_deref().map(|x| ParseErrorMessage::Expected(x))
                        },
                        self.range_from_span(last_token.span())
                    );
                    
                    Some(Err(node))

                } else {
                    self.advance_until_core_loop(allow_list, construct_delimiters, last_token.start(), token.end())
                }
            },

            // Push a missing token error as we reached the
            // end of the file without finding an expected token.
            None => {
                // Pushes an error for the previous error node.
                self.ast_errors.push(
                    ParseError::UnexpectedTokens {
                        msg: allow_list.to_string().as_deref().map(|x| ParseErrorMessage::Expected(x))
                    },
                    self.range_from_span(last_token.span())
                );
                
                None
            }
        }

    }

    fn optional_node_is_kind_else_advance_until<const N: usize>(
        &mut self,
        node: Option<Node<'a>>,
        allow_list: &TokenKindList<N>,
        construct_delimiters: &LazyLock<HashSet<TokenKind>>
    ) -> Option<SymResult<Node<'a>>> {
        match node {
            Some(node) => self.node_is_kind_else_advance_until(node, allow_list, construct_delimiters),

            None => {
                self.ast_errors.push(
                    ParseError::MissingToken {
                        msg: allow_list.to_string().as_deref().map(|x| ParseErrorMessage::Expected(x))
                    },
                    self.range_from_span(clamp_span_to_end(self.last_token_end))
                );

                None
            }
        }
    }

    fn parse_datatype(
        &mut self,
        node: Option<Node<'a>>,
        construct_delimiters: LazyLock<HashSet<TokenKind>>
    ) -> (NodeStatus<'a>, Option<Construct<'a>>) {
        let (node_status, construct) = self.parse_datatype_part(node, &construct_delimiters);

        if let Some(construct) = construct {
            let middle_node = match node_status {
                NodeStatus::Exists => self.advance(),
                NodeStatus::None | NodeStatus::Err(_) => return (node_status, Some(construct)),
            };

            if let Some(some_middle_node) = middle_node {
                if let Some(precedence) = TOKEN_KIND_OPERATOR_PRECEDENCE.get(&some_middle_node.token.value().kind()) {
                    // We are parsing an operation.

                    let (right_node, operators) =
                        self.parse_datatype_operators(some_middle_node, *precedence);

                    if node_token_matches!(right_node, Some(SemiColon)) {
                        self.ast_errors.push(
                            ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected("a datatype")) },
                            self.range_from_span(clamp_span_to_end(operators.last().unwrap().token.end()))
                        );

                        return (right_node.to_status(), Some(Construct::MathOperation {
                            left: Box::new(construct), operators: operators, right: None
                        }))
                    }

                    self.parse_datatype_operation(
                        right_node, construct,
                        *precedence, operators,
                        &construct_delimiters
                    )

                } else { (some_middle_node.to_status(), Some(construct)) }

            } else { (middle_node.to_status(), Some(construct)) }

        } else { (node_status, None) }
    }

    fn parse_datatype_part(
        &mut self, node: Option<Node<'a>>,
        construct_delimiters: &LazyLock<HashSet<TokenKind>>
    ) -> (NodeStatus<'a>, Option<Construct<'a>>) {
        let node = guarded_unwrap_advance!(
            self.optional_node_is_kind_else_advance_until(
                node, token_kind_list!("a datatype", [
                    Identifier, ParensOpen,
                    StringMulti, StringSingle,
                    Number, NumberScale, NumberOffset,
                    Boolean, Nil,
                    StaticTokenIdentifier, TokenIdentifier,
                    ColorHex, ColorTailwind, ColorCss, ColorBrick,
                    RbxAsset, RbxContent,
                    EnumKeyword, StateSelectorOrEnumPart,
                    MacroCallIdentifier
                ]),
                construct_delimiters
            ),
            return (NodeStatus::.., None)
        );

        let token = &node.token;

        match token.value() {
            Token::Identifier(_) => self.parse_annotated_table_datatype(node),

            Token::ParensOpen => self.parse_table_datatype(node),

            Token::EnumKeyword => self.parse_enum_datatype(node),

            Token::MacroCallIdentifier(_) => {
                let Parsed (node, construct) = self.parse_macro_call_body(node);
                (node.to_status(), construct)
            },

            Token::StringMulti(MultilineString { nestedness: Err(expected_nestedness), .. }) => {
                self.handle_multiline_string_error(&token, *expected_nestedness);

                (NodeStatus::Exists, Some(Construct::Node { node }))
            },

            _ => (NodeStatus::Exists, Some(Construct::Node { node }))
        }
    }

    fn parse_datatype_operators(
        &mut self, some_middle_node: Node<'a>, precedence: usize
    ) -> (Option<Node<'a>>, Vec<Node<'a>>) {
        let mut operators = vec![some_middle_node];
        let right_node = if precedence == TOKEN_KIND_ADD_SUB_PRECEDENCE {

            // Chains consecuative Add and Sub operators.
            loop {
                let middle_node = self.advance(); 

                if let Some(some_middle_node) = middle_node {
                    if let Some(precedence) =
                        TOKEN_KIND_OPERATOR_PRECEDENCE.get(&some_middle_node.token.value().kind()) 
                    {
                        if *precedence == TOKEN_KIND_ADD_SUB_PRECEDENCE {
                            operators.push(some_middle_node);

                        } else {
                            self.ast_errors.push(
                                ParseError::UnexpectedTokens { msg: None },
                                self.range_from_span(clamp_span_to_end(some_middle_node.token.end()))
                            );
                        }

                    } else { break Some(some_middle_node) }

                } else { break middle_node }
            }
        } else { self.advance() };

        (right_node, operators)
    }

    fn parse_datatype_operation(
        &mut self,
        node: Option<Node<'a>>,
        last_datatype: Construct<'a>,
        last_precedence: usize,
        last_operators: Vec<Node<'a>>,
        construct_delimiters: &LazyLock<HashSet<TokenKind>>
    ) -> (NodeStatus<'a>, Option<Construct<'a>>) {
        let (node_status, construct) = self.parse_datatype_part(node, construct_delimiters);

        if let Some(construct) = construct {
            let middle_node = match node_status {
                NodeStatus::Exists => self.advance(),
                NodeStatus::None | NodeStatus::Err(_) => return (node_status, Some(Construct::MathOperation {
                    left: Box::new(last_datatype), operators: last_operators, right: Some(Box::new(construct))
                })),
            };

            if let Some(some_middle_node) = middle_node {
                if let Some(precedence) = TOKEN_KIND_OPERATOR_PRECEDENCE.get(&some_middle_node.token.value().kind()) {
                    // We are parsing an operation.

                    let (right_node, operators) =
                        self.parse_datatype_operators(some_middle_node, *precedence);

                    // We have reached the end of the operation.
                    if node_token_matches!(right_node, Some(SemiColon)) {
                        self.ast_errors.push(
                            ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected("a datatype")) },
                            self.range_from_span(clamp_span_to_end(operators.last().unwrap().token.end()))
                        );

                        return if *precedence > last_precedence {
                            (right_node.to_status(), Some(Construct::MathOperation {
                                left: Box::new(last_datatype),
                                operators: last_operators,
                                right: Some(Box::new(Construct::MathOperation {
                                    left: Box::new(construct),
                                    operators,
                                    right: None
                                }))
                            }))

                        } else {
                            (right_node.to_status(), Some(Construct::MathOperation {
                                left: Box::new(Construct::MathOperation {
                                    left: Box::new(last_datatype),
                                    operators: last_operators,
                                    right: Some(Box::new(construct))
                                }),
                                operators,
                                right: None
                            }))
                        }
                    }

                    if *precedence > last_precedence {
                        let (node_status, construct) = self.parse_datatype_operation(
                            right_node, construct,
                            *precedence, operators,
                            construct_delimiters
                        );

                        return (node_status, Some(Construct::MathOperation {
                            left: Box::new(last_datatype),
                            operators: last_operators,
                            right: construct.map(Box::new)
                        }))

                    } else {
                        return self.parse_datatype_operation(
                            right_node,
                            Construct::MathOperation {
                                left: Box::new(last_datatype),
                                operators: last_operators,
                                right: Some(Box::new(construct))
                            },
                            *precedence, operators,
                            construct_delimiters
                        )
                    }

                } else {
                    (some_middle_node.to_status(), Some(Construct::MathOperation {
                        left: Box::new(last_datatype), operators: last_operators, right: Some(Box::new(construct))
                    }))
                }

            } else {
                (middle_node.to_status(), Some(Construct::MathOperation {
                    left: Box::new(last_datatype), operators: last_operators, right: Some(Box::new(construct))
                }))
            }

        } else {
            (node_status, Some(Construct::MathOperation {
                left: Box::new(last_datatype), operators: last_operators, right: construct.map(Box::new)
            }))
        }
    }

    /// Main routine for parsing table datatype arguments.
    /// Returns an Err or None if we have reached the end of the table.
    fn parse_table_datatype_arg_main(
        &mut self,
        this_node: Option<Node<'a>>,
        datatype_group: Construct<'a>,
        datatype_groups: &mut Vec<Construct<'a>>
    ) -> Option<SymResult<Node<'a>>> {
        let this_node = guarded_unwrap!(this_node, return {
            datatype_groups.push(datatype_group);
            None
        });

        match this_node.token.value() {
            Token::Comma => {
                let next_node = self.advance();

                if let Some(next_node) = next_node {
                    let next_token_value = next_node.token.value();

                    // We have reached the end of the table.
                    if matches!(next_token_value, Token::ParensClose) {
                        datatype_groups.push(datatype_group);

                        // Pushes an error for the trailing comma.
                        self.ast_errors.push(
                            ParseError::UnexpectedTokens { msg: None },
                            self.range_from_span(this_node.token.span())
                        );

                        Some(Err(next_node))

                    } else if TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS.contains(&next_token_value.kind()) {
                        Some(Err(next_node))

                    } else {
                        datatype_groups.reserve(2);
                        datatype_groups.push(datatype_group);
                        datatype_groups.push(Construct::Node { node: this_node });

                        Some(Ok(next_node))
                    }
                    
                } else {
                    datatype_groups.reserve(2);
                    datatype_groups.push(datatype_group);
                    datatype_groups.push(Construct::Node { node: this_node });

                    None
                }
            },

            Token::ParensClose => {
                datatype_groups.push(datatype_group);

                Some(Err(this_node))
            },

            token => {
                if TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS.contains(&token.kind()) {
                    datatype_groups.push(datatype_group);

                    Some(Err(this_node))
                    
                } else {
                    // Pushes an error for the trailing comma.
                    self.ast_errors.push(
                        ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::Comma.name())) },
                        self.range_from_span(clamp_span_to_end(datatype_group.end()))
                    );

                    datatype_groups.push(datatype_group);

                    Some(Ok(this_node))
                }
            }
        }
    }

    fn parse_table_datatype_args(&mut self, mut node: Option<Node<'a>>) -> (Option<Node<'a>>, Option<Vec<Construct<'a>>>) {
        let (this_node_status, datatype_group) =
            self.parse_datatype(node, TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS);

        if let Some(datatype_group) = datatype_group {
            let mut datatype_groups = vec![];

            let this_node = this_node_status.consume_err_or_advance(self);
            node = match self.parse_table_datatype_arg_main(this_node, datatype_group, &mut datatype_groups) {
                Some(Ok(node)) => Some(node),
                Some(Err(node)) => return (Some(node), Some(datatype_groups)),
                None => return (None, Some(datatype_groups))
            };

            loop {
                let (this_node_status, datatype_group) =
                    self.parse_datatype(node, TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS);

                let this_node = this_node_status.consume_err_or_advance(self);
                node = if let Some(datatype_group) = datatype_group {
                    match self.parse_table_datatype_arg_main(this_node, datatype_group, &mut datatype_groups) {
                        Some(Ok(node)) => Some(node),
                        Some(Err(node)) => return (Some(node), Some(datatype_groups)),
                        None => return (None, Some(datatype_groups))
                    }
                } else { break (None, None) }
            }
        } else { (None, None) }
    }

    fn parse_table_datatype(&mut self, table_open_node: Node<'a>) -> (NodeStatus<'a>, Option<Construct<'a>>) {
        let node = if let Some(node) = self.advance() {
            let token_value = node.token.value();

            // We have reached the end of the table.
            if matches!(token_value, Token::ParensClose) {
                return (NodeStatus::Exists, Some(Construct::Table {
                    body: Delimited::new(table_open_node, None, Some(node))
                }))

            } else if TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS.contains(&token_value.kind()) {
                self.ast_errors.push(
                    ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ParensClose.name())) },
                    self.range_from_span(clamp_span_to_end(table_open_node.token.end()))
                );

                return (NodeStatus::Err(node), Some(Construct::Table {
                    body: Delimited::new(table_open_node, None, None)
                }))

            } else { node }

        } else {
            self.ast_errors.push(
                ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ParensClose.name())) },
                self.range_from_span(clamp_span_to_end(table_open_node.token.end()))
            );

            return (NodeStatus::None, Some(Construct::Table {
                body: Delimited::new(table_open_node, None, None)
            }))
        };

        let (node, datatype_groups) = self.parse_table_datatype_args(Some(node));

        // We need to throw an error if the node's token is not ParensClose.
        if !node_token_matches!(node, Some(ParensClose)) {
            let construct = Construct::Table {
                body: Delimited::new(table_open_node, datatype_groups, None)
            };

            self.ast_errors.push(
                ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ParensClose.name())) },
                self.range_from_span(clamp_span_to_end(construct.end()))
            );

            return (node.to_status(), Some(construct))
        }

        (NodeStatus::Exists, Some(Construct::Table {
            body: Delimited::new(table_open_node, datatype_groups, node)
        }))
    }

    fn parse_annotated_table_datatype(&mut self, annotation_node: Node<'a>) -> (NodeStatus<'a>, Option<Construct<'a>>) {
        let table_open_node = match self.advance() {
            Some(node @ Node { token: SpannedToken(_, Token::ParensOpen, _), .. }) => node,

            Some(node) => {
                // We don't want to include the trailing semi-colon in the error.
                let error_span =
                    if node_token_matches!(node, SemiColon) { annotation_node.token.span() }
                    else { (annotation_node.token.start(), node.token.end()) };

                self.ast_errors.push(
                    ParseError::UnexpectedTokens { msg: Some(ParseErrorMessage::Expected("a datatype")) },
                    self.range_from_span(error_span)
                );

                return (NodeStatus::Err(node), None);
            },

            None => {
                self.ast_errors.push(
                    ParseError::UnexpectedTokens { msg: Some(ParseErrorMessage::Expected("a datatype")) },
                    self.range_from_span(annotation_node.token.span())
                );

                return (NodeStatus::None, None)
            }
        };

        let node = if let Some(node) = self.advance() {
            let token_value = node.token.value();

            // We have reached the end of the table.
            if matches!(token_value, Token::ParensClose) {
                return (NodeStatus::Exists, Some(Construct::AnnotatedTable {
                    annotation: annotation_node,
                    body: Some(Delimited::new(table_open_node, None, Some(node)))
                }))

            } else if TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS.contains(&token_value.kind()) {
                self.ast_errors.push(
                    ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ParensClose.name())) },
                    self.range_from_span(clamp_span_to_end(table_open_node.token.end()))
                );

                return (NodeStatus::Err(node), Some(Construct::AnnotatedTable {
                    annotation: annotation_node,
                    body: Some(Delimited::new(table_open_node, None, None))
                }))

            } else { node }

        } else {
            self.ast_errors.push(
                ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ParensClose.name())) },
                self.range_from_span(clamp_span_to_end(table_open_node.token.end()))
            );

            return (NodeStatus::None, Some(Construct::AnnotatedTable {
                annotation: annotation_node,
                body: Some(Delimited::new(table_open_node, None, None))
            }))
        };

        let (node, datatype_groups) = self.parse_table_datatype_args(Some(node));

        // We need to throw an error if the node's token is not ParensClose.
        if !node_token_matches!(node, Some(ParensClose)) {
            let construct = Construct::AnnotatedTable {
                annotation: annotation_node,
                body: Some(Delimited::new(table_open_node, datatype_groups, None))
            };

            self.ast_errors.push(
                ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ParensClose.name())) },
                self.range_from_span(clamp_span_to_end(construct.end()))
            );

            return (node.to_status(), Some(construct))
        }

        (NodeStatus::Exists, Some(Construct::AnnotatedTable {
            annotation: annotation_node,
            body: Some(Delimited::new(table_open_node, datatype_groups, node))
        }))
    }

    fn parse_enum_datatype(&mut self, keyword_node: Node<'a>) -> (NodeStatus<'a>, Option<Construct<'a>>) {
        let name_node = guarded_unwrap_advance!(
            self.advance_until(
                token_kind_list!("enum part", [ StateSelectorOrEnumPart, TagSelectorOrEnumPart ]),
                &TOKEN_KIND_CONSTRUCT_DELIMITERS
            ),
            return (NodeStatus::.., Some(Construct::Enum {
                keyword: keyword_node, name: None, variant: None
            }))
        );

        let variant_node = guarded_unwrap_advance!(
            self.advance_until(
                token_kind_list!("enum part", [ StateSelectorOrEnumPart, TagSelectorOrEnumPart ]),
                &TOKEN_KIND_CONSTRUCT_DELIMITERS
            ),
            return (NodeStatus::.., Some(Construct::Enum {
                keyword: keyword_node, name: Some(name_node), variant: None
            }))
        );

        (self.advance().to_status(), Some(Construct::Enum {
            keyword: keyword_node, name: Some(name_node), variant: Some(variant_node)
        }))
    }

    /// Many declarations in rsml just have a datatype after them.
    /// So we can use the same function to parse them.
    fn parse_declaration_with_datatype(
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
            NodeStatus::Exists => guarded_unwrap_advance!(
                self.advance_until(token_kind_list![ SemiColon ], &TOKEN_KIND_CONSTRUCT_DELIMITERS),
                return Parsed (.., Some(constructor(
                    declaration_node, body_nodes, None
                )))
            ),

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

    fn parse_derive(
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

    fn parse_priority(
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

    fn parse_name(
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

    // TODO: properly implement macros.
    fn parse_macro_call(
        &mut self, node: Node<'a>
    ) -> Parsed<'a> {
        if !node_token_matches!(node, MacroCallIdentifier(_)) { return Parsed (Some(node), None) }
        
        self.parse_macro_call_body(node)
    }

    fn parse_macro_call_body(&mut self, name_node: Node<'a>) -> Parsed<'a> {
        let open_node = guarded_unwrap_advance!(
            self.advance_until(token_kind_list![ ParensOpen ], &TOKEN_KIND_CONSTRUCT_DELIMITERS),
            return Parsed (.., Some(Construct::MacroCall { name: name_node, body: None, terminator: None }))
        );

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

        let terminator_node = guarded_unwrap_advance!(
            self.advance_until(token_kind_list![SemiColon], &TOKEN_KIND_CONSTRUCT_DELIMITERS),
            return Parsed (.., Some(Construct::MacroCall {
                name: name_node,
                body: Some(Delimited::new(open_node, Some(body_content), Some(close_node))),
                terminator: None
            }))
        );

        Parsed (self.advance(), Some(Construct::MacroCall {
            name: name_node,
            body: Some(Delimited::new(open_node, Some(body_content), Some(close_node))),
            terminator: Some(terminator_node)
        }))
    }

    fn parse_macro(&mut self, node: Node<'a>) -> Parsed<'a> {
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

        let args_or_body_node = guarded_unwrap_advance!(
            self.advance_until(
                token_kind_list!("macro arguments or body", [ ScopeOpen, ParensOpen ]),
                &TOKEN_KIND_CONSTRUCT_DELIMITERS
            ),
            return Parsed (.., Some(
                Construct::Macro { declaration: declaration_node, name: name_node, args: None, body: None }
            ))
        );

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
        let mut node = guarded_unwrap_advance!(
            self.advance_until(token_kind_list![
                MacroArgIdentifier, Comma, ParensClose
            ], &TOKEN_KIND_INSIDE_PARENS_CONSTRUCT_DELIMITERS),
            return Parsed (.., Some(
                Construct::Macro {
                    declaration: declaration_node,
                    name: name_node,
                    args: Some(Delimited { left: args_open_node, content: None, right: None }),
                    body: None
                }
            ))
        );

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

            node = guarded_unwrap_advance!(advance_until_result,
                // We return early as no expected nodes were found.
                return Parsed (.., Some(
                    Construct::Macro {
                        declaration: declaration_node,
                        name: name_node,
                        args: Some(Delimited::new(args_open_node, Some(args), None)),
                        body: None
                    }
                ))
            );

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
        let body_node = guarded_unwrap_advance!(
            self.advance_until(token_kind_list![ ScopeOpen ], &TOKEN_KIND_CONSTRUCT_DELIMITERS),
            return Parsed (.., Some(
                Construct::Macro {
                    declaration: declaration_node,
                    name: name_node,
                    args: Some(Delimited { left: args_open_node, content: args_content_node, right: args_close_node }),
                    body: None
                }
            ))
        );

        return self.parse_macro_body(
            body_node,
            declaration_node,
            name_node,
            Some(Delimited::new(args_open_node, args_content_node, args_close_node))
        )
    }

    fn parse_macro_body(
        &mut self,
        body_open_node: Node<'a>,
        declaration_node: Node<'a>,
        name_node: Option<Node<'a>>,
        args_node: Option<Delimited<'a>>
    ) -> Parsed<'a> {
        let node = guarded_unwrap!(self.advance(), return {
            self.ast_errors.push(
                ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ScopeClose.name())) },
                self.range_from_span(clamp_span_to_end(body_open_node.token.end()))
            );
            Parsed (None, Some(Construct::Macro {
                declaration: declaration_node,
                name: name_node,
                args: args_node,
                body: Some(Delimited::new(body_open_node, None, None))
            }))
        });

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

    fn parse_invalid_declaration(&mut self, node: Node<'a>) -> Option<Node<'a>> {
        let token = &node.token;

        let name =
            if let Token::InvalidDeclaration(x) = token.value() { x }
            else { return Some(node) };

        self.ast_errors.push(
            ParseError::UnexpectedTokens{
                msg: Some(ParseErrorMessage::correction(
                    name.as_deref().map(|x| format!("@{x}")),
                    self.range_from_span((token.start(), token.end())),
                    &DECLARATION_NAMES
                ))
            },
            self.range_from_span((token.start(), token.end()))
        );

        self.advance()
    }

    fn parse_none(&mut self, node: Node<'a>) -> Parsed<'a> {
        if !node_token_matches!(node, None) { return Parsed (Some(node), None) };

        Parsed (self.advance(), Some(Construct::None { node }))
    }

    fn parse_assignment(&mut self, node: Node<'a>) -> Parsed<'a> {
        let middle_node = guarded_unwrap_advance!(
            self.advance_until(token_kind_list![ Equals ], &TOKEN_KIND_CONSTRUCT_DELIMITERS),
            return Parsed (.., None)
        );

        let left_node = node;

        let node = self.advance_without_flags();
        self.did_advance = true;

        let (node_status, body_nodes) =
            self.parse_datatype(node, TOKEN_KIND_CONSTRUCT_DELIMITERS);
        let body_nodes = body_nodes.map(|x| Box::new(x));

        let terminator = match node_status {
            NodeStatus::Exists => guarded_unwrap_advance!(
                self.advance_until(token_kind_list![ SemiColon ], &TOKEN_KIND_CONSTRUCT_DELIMITERS),
                return Parsed (.., Some(Construct::Assignment {
                    left: left_node, middle: Some(middle_node), right: body_nodes, terminator: None
                }))
            ),

            NodeStatus::Err(node) => {
                if node_token_matches!(node, SemiColon) {
                    node

                } else {
                    let construct = Construct::Assignment {
                        left: left_node, middle: Some(middle_node), right: body_nodes, terminator: None
                    };

                    self.ast_errors.push(
                        ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::SemiColon.name())) },
                        self.range_from_span(clamp_span_to_end(construct.end()))
                    );

                    return Parsed (Some(node), Some(construct))
                }
            },

            NodeStatus::None => {
                let construct = Construct::Assignment {
                    left: left_node, middle: Some(middle_node), right: body_nodes, terminator: None
                };

                self.ast_errors.push(
                    ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::SemiColon.name())) },
                    self.range_from_span(clamp_span_to_end(construct.end()))
                );

                return Parsed (None, Some(construct))
            }
        };
           
        Parsed (self.advance(), Some(Construct::Assignment {
            left: left_node, middle: Some(middle_node), right: body_nodes, terminator: Some(terminator)
        }))
    }

    fn parse_static_token_assignment(&mut self, node: Node<'a>) -> Parsed<'a> {
        if !node_token_matches!(node, StaticTokenIdentifier(_)) { return Parsed (Some(node), None) };
        self.parse_assignment(node)
    }

    fn parse_token_assignment(&mut self, node: Node<'a>) -> Parsed<'a> {
        if !node_token_matches!(node, TokenIdentifier(_)) { return Parsed (Some(node), None) };
        self.parse_assignment(node)
    }

    fn parse_property_assignment_or_rule_scope(&mut self, node: Node<'a>) -> Parsed<'a> {
        if !node_token_matches!(node, Identifier(_)) { return Parsed (Some(node), None) };
        
        let middle_node = match self.advance_until(
            &token_kind_list!(
                "property assignment, selector part or rule body", [
                    Equals, ScopeOpen, Identifier, NameSelector,
                    TagSelectorOrEnumPart, StateSelectorOrEnumPart, 
                    PseudoSelector, Comma, ChildrenSelector, DescendantsSelector
                ]
            ),
            &TOKEN_KIND_CONSTRUCT_DELIMITERS
        ) {
            Some(Ok(node)) => node,
            Some(Err(node)) => return Parsed (Some(node), None),
            None => return Parsed (None, None)
        };

        let middle_token_value = middle_node.token.value();

        // We switch to parsing a selector if the token is not an equals sign. 
        if !matches!(middle_token_value, Token::Equals) {
            return match middle_token_value {
                Token::ScopeOpen => self.parse_rule_scope_body(middle_node, Some(vec![node])),

                Token::Comma => {
                    let token = middle_node.token.clone();
                    self.parse_rule_scope_selector(token, vec![node, middle_node])
                },

                _ => {
                    let token = middle_node.token.clone();
                    self.parse_rule_scope_selector_delimited(token,  vec![node, middle_node])
                }
            }
        }

        let left_node = node;

        let node = self.advance_without_flags();
        self.did_advance = true;

        let (node_status, body_nodes) =
            self.parse_datatype(node, TOKEN_KIND_CONSTRUCT_DELIMITERS);
        let body_nodes = body_nodes.map(|x| Box::new(x));

        let terminator = match node_status {
            NodeStatus::Exists => guarded_unwrap_advance!(
                self.advance_until(token_kind_list![ SemiColon ], &TOKEN_KIND_CONSTRUCT_DELIMITERS),
                return Parsed (.., Some(Construct::Assignment {
                    left: left_node, middle: Some(middle_node), right: body_nodes, terminator: None
                }))
            ),

            NodeStatus::Err(node) => {
                if node_token_matches!(node, SemiColon) {
                    node

                } else {
                    let construct = Construct::Assignment {
                        left: left_node, middle: Some(middle_node), right: body_nodes, terminator: None
                    };

                    self.ast_errors.push(
                        ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::SemiColon.name())) },
                        self.range_from_span(clamp_span_to_end(construct.end()))
                    );

                    return Parsed (Some(node), Some(construct))
                }
            },

            NodeStatus::None => {
                let construct = Construct::Assignment {
                    left: left_node, middle: Some(middle_node), right: body_nodes, terminator: None
                };

                self.ast_errors.push(
                    ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::SemiColon.name())) },
                    self.range_from_span(clamp_span_to_end(construct.end()))
                );

                return Parsed (None, Some(construct))
            }
        };

        Parsed (self.advance(), Some(Construct::Assignment {
            left: left_node, middle: Some(middle_node), right: None, terminator: Some(terminator)
        }))
    }

    fn parse_rule_scope_selector_begin(&mut self, node: Node<'a>) -> Parsed<'a> {
        let node = match node.token.value() {
            Token::NameSelector(_) | Token::TagSelectorOrEnumPart(_) | 
            Token::StateSelectorOrEnumPart(_) | Token::PseudoSelector(_) | Token::ChildrenSelector |
            Token::DescendantsSelector => node,

            Token::ScopeOpen => return self.parse_rule_scope_body(node, None),

            _ => return Parsed(Some(node), None)
        };

        let token = node.token.clone();

        self.parse_rule_scope_selector_delimited(token, vec![node])
    }

    fn parse_rule_scope_selector(
        &mut self, last_token: SpannedToken<'a>, mut selectors: Vec<Node<'a>>
    ) -> Parsed<'a> {
        let node = guarded_unwrap_advance!(
            self.advance_until(token_kind_list!("selector part or \"{\"", [
                Identifier, NameSelector, TagSelectorOrEnumPart, StateSelectorOrEnumPart, PseudoSelector,
                ChildrenSelector, DescendantsSelector, ScopeOpen
            ]), &TOKEN_KIND_CONSTRUCT_DELIMITERS), return Parsed (.., Some(Construct::Rule { selectors, body: None }))
        );

        self.handle_hierarchy_selector_without_part(&last_token, &node.token);

        if node_token_matches!(node, ScopeOpen) {
            // Pushes an error for a trailing comma.
            if matches!(last_token.value(), Token::Comma) {
                self.ast_errors.push(
                    ParseError::UnexpectedTokens { msg: None },
                    self.range_from_span(last_token.span())
                );
            }

            return self.parse_rule_scope_body(node, Some(selectors))
        }

        let token = node.token.clone();
        selectors.push(node);

        self.parse_rule_scope_selector_delimited(token, selectors)
    }

    fn parse_rule_scope_selector_delimited(
        &mut self, last_token: SpannedToken<'a>, mut selectors: Vec<Node<'a>>
    ) -> Parsed<'a> {
        let node = guarded_unwrap_advance!(
            self.advance_until(token_kind_list!("selector part or \"{\"", [
                Identifier, NameSelector, TagSelectorOrEnumPart, StateSelectorOrEnumPart, PseudoSelector,
                ChildrenSelector, DescendantsSelector, ScopeOpen, Comma
            ]), &TOKEN_KIND_CONSTRUCT_DELIMITERS), return Parsed (.., Some(Construct::Rule { selectors, body: None }))
        );

        self.handle_hierarchy_selector_without_part(&last_token, &node.token);

        if node_token_matches!(node, ScopeOpen) { return self.parse_rule_scope_body(node, Some(selectors)) }

        let token = node.token.clone();
        selectors.push(node);


        match token.value() {
            Token::Comma => self.parse_rule_scope_selector(token, selectors),
            _ => self.parse_rule_scope_selector_delimited(token, selectors)
        }
    }

    fn handle_hierarchy_selector_without_part(&mut self, last_token: &SpannedToken<'a>, token: &SpannedToken<'a>) {
        if !(
            matches!(last_token.value(), Token::DescendantsSelector | Token::ChildrenSelector) &&
            matches!(
                token.value(),
                Token::DescendantsSelector | Token::ChildrenSelector | Token::Comma | Token::ScopeOpen
            ) 
        ) { return }

        self.ast_errors.push(
            ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected("selector part")) },
            self.range_from_span(clamp_span_to_end(last_token.end()))
        );
    }

    fn parse_rule_scope_body(&mut self, body_open_node: Node<'a>, selectors: Option<Vec<Node<'a>>>) -> Parsed<'a> {
        let node = guarded_unwrap!(self.advance(), return {
            self.ast_errors.push(
                ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ScopeClose.name())) },
                self.range_from_span(clamp_span_to_end(body_open_node.token.end()))
            );
            Parsed (None, Some(Construct::rule(selectors, Delimited::new(body_open_node, None, None))))
        });

        if node_token_matches!(node, ScopeClose) {
            return Parsed (self.advance(), Some(Construct::rule(
                selectors,
                Delimited::new(body_open_node, None, Some(node))
            )))
        }

        let mut body_content: Vec<Construct<'a>> = vec![];

        let (node, parse_ended_reason) =
            self.parse_loop_inner(node,|parser, mut node| {
                node = parser.parse_macro(node).handle_construct_with_err(
                    &mut body_content, &mut parser.ast_errors, &parser.lexer.rope, Some("rules")
                )?;

                node = parser.parse_macro_call(node).handle_construct(&mut body_content)?;

                node = parser.parse_derive(node).handle_construct_with_err(
                    &mut body_content, &mut parser.ast_errors, &parser.lexer.rope, Some("non-global scopes")
                )?;

                node = parser.parse_priority(node).handle_construct(&mut body_content)?;
                node = parser.parse_name(node).handle_construct(&mut body_content)?;

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
            return Parsed (self.advance(), Some(Construct::rule(
                selectors,
                Delimited::new(body_open_node, Some(body_content), node)
            )))

        // We push an error as there is no closing curly brace.
        } else {
            let construct = Construct::rule(
                selectors,
                Delimited::new(body_open_node, Some(body_content), None)
            );

            self.ast_errors.push(
                ParseError::MissingToken { msg: Some(ParseErrorMessage::Expected(TokenKind::ScopeClose.name())) },
                self.range_from_span(clamp_span_to_end(construct.end()))
            );

            Parsed (self.advance(), Some(construct))
        }
    }

    fn parse_loop<F: Fn(&mut Self, Node<'a>) -> Option<Node<'a>>>(&mut self, routine: F) -> Option<Node<'a>> {
        let mut node = self.advance_without_flags()
            .update_last_token_end(self)?;
        let token = &node.token;

        let mut error_span: Option<(usize, usize)> = 
            if matches!(token.value(), Token::Error) { Some((token.start(), token.end())) } else { None };
        
        loop {
            node = guarded_unwrap!(routine(self, node), break);

            if self.did_advance {
                self.did_advance = false;

                // Now that we have advanced we can push all of the
                // previously skipped tokens (if any) as an error.
                if let Some((error_span_start, error_span_end)) = error_span {
                    self.ast_errors.push(
                        ParseError::UnexpectedTokens { msg: None },
                        self.range_from_span((error_span_start, error_span_end))
                    );
                }

            } else {
                let token = &node.token;

                if let Some((error_span_start, _)) = error_span {
                    // Adjusts the existing error span to accomodate for this skipped token.
                    error_span = Some((error_span_start, token.end()))
                } else {
                    // Creates a new error span to accomodate for this skipped token.
                    error_span = Some((token.start(), token.end()))
                }
                
                node = guarded_unwrap!(
                    self.advance_without_flags()
                        .update_last_token_end(self),
                    break
                )
            }
        }

        // Now that we have reached the end of the document we can push 
        // all of the previously skipped tokens (if any) as an error.
        if let Some((error_span_start, error_span_end)) = error_span {
            self.ast_errors.push(
                ParseError::UnexpectedTokens { msg: None },
                self.range_from_span((error_span_start, error_span_end))
            );
        }

        None
    }

    fn parse_loop_inner<F: FnMut(&mut Self, Node<'a>) -> Option<(Node<'a>, bool)>>(
        &mut self, mut node: Node<'a>, mut routine: F
    ) -> (Option<Node<'a>>, ParseEndedReason) {
        let last_did_advance = self.did_advance;
        self.did_advance = false;

        let mut error_span: Option<(usize, usize)> = None;

        loop {
            let parsed = guarded_unwrap!(
                routine(self, node),
                return {
                    self.did_advance = last_did_advance;
                    (None, ParseEndedReason::Eof)
                }
            );
            node = parsed.0;

            if self.did_advance {
                self.did_advance = false;

                // Now that we have advanced we can push the
                // previously skipped tokens (if any) as an error.
                if let Some((error_span_start, error_span_end)) = error_span {
                    self.ast_errors.push(
                        ParseError::UnexpectedTokens { msg: None },
                        self.range_from_span((error_span_start, error_span_end))
                    );
                }

                if parsed.1 { return (Some(node), ParseEndedReason::Manual) }

            } else {
                if parsed.1 {
                    // Becuase we are terminating early we need to push
                    // the previously skipped tokens (if any) as an error.
                    if let Some((error_span_start, error_span_end)) = error_span {
                        self.ast_errors.push(
                            ParseError::UnexpectedTokens { msg: None },
                            self.range_from_span((error_span_start, error_span_end))
                        );
                    }

                    return (Some(node), ParseEndedReason::Manual)
                }

                let token = &node.token;
                if let Some((error_span_start, _)) = error_span {
                    // Adjusts the existing error span to accomodate for this skipped token.
                    error_span = Some((error_span_start, token.end()))
                } else {
                    // Creates a new error span to accomodate for this skipped token.
                    error_span = Some((token.start(), token.end()))
                }

                node = guarded_unwrap!(
                    self.advance_without_flags()
                        .update_last_token_end(self),
                    break
                )
            }
        };

        // Now that we have reached the end of the document we can push 
        // all of the previously skipped tokens (if any) as an error.
        if let Some((error_span_start, error_span_end)) = error_span {
            self.ast_errors.push(
                ParseError::UnexpectedTokens { msg: None },
                self.range_from_span((error_span_start, error_span_end))
            );
        }

        self.did_advance = last_did_advance;

        (Some(node), ParseEndedReason::Eof)
    }
}


#[derive(Debug)]
pub enum Construct<'a> {
    Macro {
        declaration: Node<'a>,
        name: Option<Node<'a>>,
        args: Option<Delimited<'a>>,
        body: Option<Delimited<'a>>
    },

    MacroCall {
        name: Node<'a>,
        body: Option<Delimited<'a>>,
        terminator: Option<Node<'a>>
    },

    Derive {
        declaration: Node<'a>,
        body: Option<Box<Construct<'a>>>,
        terminator: Option<Node<'a>>
    },

    Priority {
        declaration: Node<'a>,
        body: Option<Box<Construct<'a>>>,
        terminator: Option<Node<'a>>
    },

    Name {
        declaration: Node<'a>,
        body: Option<Box<Construct<'a>>>,
        terminator: Option<Node<'a>>
    },

    Rule {
        selectors: Vec<Node<'a>>,
        body: Option<Delimited<'a>>
    },

    RuleNoSelectors {
        body: Delimited<'a>
    },

    Assignment {
        left: Node<'a>,
        middle: Option<Node<'a>>,
        right: Option<Box<Construct<'a>>>,
        terminator: Option<Node<'a>>
    },

    MathOperation {
        left: Box<Construct<'a>>,
        operators: Vec<Node<'a>>,
        right: Option<Box<Construct<'a>>>
    },

    AnnotatedTable {
        annotation: Node<'a>,
        body: Option<Delimited<'a>>
    },

    Table {
        body: Delimited<'a>
    },

    Enum {
        keyword: Node<'a>,
        name: Option<Node<'a>>,
        variant: Option<Node<'a>>
    },

    Node { node: Node<'a> },

    None { node: Node<'a> },
}

impl<'a> Construct<'a> {
    pub fn rule(selectors: Option<Vec<Node<'a>>>, body: Delimited<'a>) -> Self {
        match selectors {
            Some(selectors) => Self::Rule { selectors, body: Some(body) },
            None => Self::RuleNoSelectors { body }
        }
    }

    pub fn name_plural(&self) -> &str {
        match self {
            Self::Macro { .. } => "Macros",
            Self::MacroCall { .. } => "Macro calls",
            Self::Derive { .. } => "Derives",
            Self::Priority { .. } => "Priorities",
            Self::Name { .. } => "Names",
            Self::Rule { .. } | Self::RuleNoSelectors { .. } => "Rules",
            Self::Assignment { left, .. } => match left.token.value() {
                Token::Identifier(_) => "Property assignments",
                Token::StaticTokenIdentifier(_) => "Static token assignments",
                Token::TokenIdentifier(_) => "Token assignments",
                _ => "Assignments"
            },
            Self::MathOperation { .. } => "Math Operations",
            Self::Table { .. } | Self::AnnotatedTable { .. } => "Tables",
            Self::Enum { .. } => "Enums",
            Self::Node { .. } | Self::None { .. } => "These"
        }
    }

    pub fn start(&self) -> usize {
        match self {
            Self::Macro { declaration, .. } => declaration.token.start(),

            Self::MacroCall { name, .. } => name.token.start(),

            Self::Derive { declaration, .. } |
            Self::Priority { declaration, .. } |
            Self::Name { declaration, .. } => declaration.token.start(),

            Self::Rule { selectors, body } => {
                selectors.first().map(|x| x.token.start())
                    .unwrap_or_else(||
                        body.as_ref().map(|x| x.left.token.start())
                            .unwrap_or_else(|| 0)
                    )
            },

            Self::RuleNoSelectors { body } => body.start(),

            Self::Assignment { left, .. } => left.token.start(),

            Self::MathOperation { left, .. } => left.start(),

            Self::AnnotatedTable { annotation, .. } => annotation.token.start(),
            Self::Table { body } => body.start(),

            Self::Enum { keyword, .. } => keyword.token.start(),

            Self::Node { node } | Self::None { node } => node.token.start(),
        }
    }

    pub fn span(&self) -> (usize, usize) {
        (self.start(), self.end())
    }
}

impl<'a> SpanEnd for Construct<'a> {
    fn end(&self) -> usize {
        match self {
            Self::Macro { declaration, name, args, body, .. } => {
                body.as_ref().map(|x| x.end())
                    .unwrap_or_else(||
                        args.as_ref().map(|x| x.end())
                            .unwrap_or_else(||
                                name.as_ref().map(|x| x.token.end())
                                    .unwrap_or_else(|| declaration.token.end())
                            )
                    )
            },

            Self::MacroCall { name, body, terminator } => {
                terminator.as_ref().map(|x| x.token.end())
                    .unwrap_or_else(||
                        body.as_ref().map(|x| x.end())
                            .unwrap_or_else(|| name.token.end())
                    )
            }

            Self::Derive { declaration, body, terminator } |
            Self::Priority { declaration, body, terminator } |
            Self::Name { declaration, body, terminator } => {
                terminator.as_ref().map(|x| x.token.end())
                    .unwrap_or_else(||
                        body.as_ref().map(
                            |x| x.end()
                        )
                            .unwrap_or_else(|| declaration.token.end())
                    )
            },

            Self::Rule { body, .. } => {
                body.as_ref().map(|x| x.end())
                    .unwrap_or_else(|| 0)
            },

            Self::RuleNoSelectors { body } => body.end(),

            Self::Assignment { left, middle, right, terminator } => {
                terminator.as_ref().map(|x| x.token.end())
                    .unwrap_or_else(||
                        right.as_ref().map(
                            |x| x.end()
                        )
                            .unwrap_or_else(
                                || middle.as_ref().map(|x| x.token.end())
                                    .unwrap_or_else(|| left.token.end())
                            )
                    )
            },

            Self::MathOperation { left, operators, right, .. } => {
                right.as_ref().map(|x| x.end())
                    .unwrap_or_else(||
                        operators.last().map(|x| x.token.end())
                            .unwrap_or_else(|| left.end())
                    )
            }

            Self::AnnotatedTable { annotation, body } => {
                body.as_ref().map(|x| x.end())
                    .unwrap_or_else(|| annotation.token.end())
            },
            Self::Table { body } => body.end(),

            Self::Enum { keyword, name, variant } => {
                variant.as_ref().map(|x| x.token.end())
                    .unwrap_or_else(||
                        name.as_ref().map(|x| x.token.end())
                            .unwrap_or_else(|| keyword.token.end())
                    )
            }

            Self::Node { node } | Self::None { node } => node.token.end(),
        }
    }
}

enum ParseEndedReason {
    Eof,
    Manual
}

#[derive(Debug)]
pub struct Delimited<'a, T: SpanEnd = Construct<'a>> {
    pub left: Node<'a>,
    pub content: Option<Vec<T>>,
    pub right: Option<Node<'a>>
}

impl<'a, T: SpanEnd> Delimited<'a, T> {
    fn new(
        left: Node<'a>,
        content: Option<Vec<T>>,
        right: Option<Node<'a>>
    ) -> Self {
        Self {
            left, content, right
        }
    }

    #[inline(always)]
    fn start(&self) -> usize {
        self.left.token.start()
    }

    fn end(&self) -> usize {
        self.right.as_ref().map(|x| x.token.2)
            .unwrap_or(
                self.content.as_ref().map(
                    |x| x.last().map(|x| x.end())
                        .unwrap_or(self.left.token.end())
                )
                .unwrap_or(self.left.token.end())
            )
    }

    fn span(&self) -> (usize, usize) {
        (self.start(), self.end())
    }
}

trait SpanEnd {
    fn end(&self) -> usize;
}

#[derive(Debug)]
pub struct AstErrors(pub Vec<Diagnostic>);

impl AstErrors {
    pub fn new() -> Self {
        Self(Vec::new())
    }
}

trait PushParseError {
    fn push(&mut self, error: ParseError, range: Range);
}

impl PushParseError for AstErrors {
    fn push(&mut self, error: ParseError, range: Range) {
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


#[inline(always)]
fn clamp_span_to_end(span_end: usize) -> (usize, usize) {
    (span_end - 1, span_end)
}

#[derive(Debug)]
pub enum NodeStatus<'a> {
    Exists,

    None,

    /// Error node when advancing until a specific token 
    /// but a block delimiter token was reached instead.
    Err(Node<'a>),
}

impl<'a> NodeStatus<'a> {
    fn consume_err_or_advance(self, parser: &mut Parser<'a>) -> Option<Node<'a>> {
        match self {
            Self::Err(node) => Some(node),
            Self::Exists => parser.advance(),
            Self::None => None
        }
    }
}