use rbx_rsml::lexer::Token;
use rbx_rsml::parser::{Construct, Delimited};
use rbx_rsml::typechecker::{DefinitionKind, Definitions};

pub fn build_rule_body_definitions(body: &Option<Delimited<'_>>, definitions: &mut Definitions) {
    let Some(body) = body.as_ref() else { return };

    let Some(content) = body.content.as_ref() else {
        return;
    };

    // The typechecker already inserted a `Scope` definition for each rule body, so
    // we look it up by body start to recover the scope's type definition.
    let body_start = body.left.token.start();

    let current_classes = definitions
        .get(&body_start)
        .and_then(|kind| {
            if let DefinitionKind::Scope { type_definition } = kind {
                Some(type_definition.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();

    // Upper bound for an Assignment whose RHS the parser didn't capture: run
    // up to the next sibling construct (e.g. a recovered nested rule from the
    // unrecognized RHS tokens) or the body's closing `}` if it was the last one.
    let body_close_start = body.right.as_ref().map(|r| r.token.start());

    for (index, construct) in content.iter().enumerate() {
        match construct {
            Construct::Rule { body, .. } => {
                build_rule_body_definitions(body, definitions);
            }

            Construct::Assignment {
                left,
                middle,
                right,
                terminator,
            } => {
                let Token::Identifier(property_name) = left.token.value() else {
                    continue;
                };

                let Some(middle) = middle else { continue };

                let assign_start = middle.token.start();
                let assign_end = terminator
                    .as_ref()
                    .map(|t| t.token.end())
                    .or_else(|| right.as_ref().map(|r| r.span().1))
                    .unwrap_or_else(|| {
                        // RHS wasn't captured by the parser — extend up to the
                        // next sibling's start (or the closing `}`) so the
                        // cursor on invalid/partial RHS text still dispatches
                        // as an assignment instead of leaking to the scope.
                        let next_start = content
                            .get(index + 1)
                            .map(|next| next.span().0.saturating_sub(1));

                        next_start
                            .or(body_close_start.map(|pos| pos.saturating_sub(1)))
                            .unwrap_or_else(|| middle.token.end())
                    });

                definitions.insert(
                    assign_start..=assign_end,
                    DefinitionKind::Assignment {
                        property_name: property_name.to_string(),
                        type_definition: current_classes.clone(),
                    },
                );

                let Some(right) = right else { continue };

                super::tokens::walk_construct(right, definitions);

                match right.as_ref() {
                    Construct::Enum {
                        keyword,
                        name,
                        variant,
                    } => {
                        let name_range_start = keyword.token.end();
                        let name_range_end = name
                            .as_ref()
                            .map(|node| node.token.end())
                            .unwrap_or(assign_end);

                        // When the property has a declared enum type (e.g.
                        // `AutomaticSize`), narrow long-form completion to that
                        // enum instead of dumping every enum in the reflection db.
                        let property_enum = super::values::property_enum_name(&current_classes, property_name);

                        let name_range_kind = match property_enum.clone() {
                            Some(enum_name) => DefinitionKind::FilteredEnumName { enum_name },
                            None => DefinitionKind::EnumName,
                        };

                        definitions.insert(name_range_start..=name_range_end, name_range_kind);

                        if let Some(name_node) = name {
                            let typed_name = match name_node.token.value() {
                                Token::TagSelectorOrEnumPart(Some(name)) => Some(*name),
                                Token::StateSelectorOrEnumPart(Some(name)) => Some(*name),
                                _ => None,
                            };

                            // Without a typed name the user is still picking an
                            // enum name, so leave the earlier FilteredEnumName /
                            // EnumName range intact instead of shadowing it.
                            if typed_name.is_none() {
                                continue;
                            }

                            let variant_enum_name = property_enum.or_else(|| typed_name.map(String::from));

                            let Some(enum_name) = variant_enum_name else {
                                continue;
                            };

                            let variant_range_start = name_node.token.end();
                            let variant_range_end = variant
                                .as_ref()
                                .map(|node| node.token.end())
                                .unwrap_or(assign_end);

                            definitions.insert(
                                variant_range_start..=variant_range_end,
                                DefinitionKind::EnumVariant { enum_name },
                            );
                        }
                    }

                    _ => (),
                }
            }

            Construct::Tween {
                body: Some(body), ..
            } => {
                let span = construct.span();
                definitions.insert(span.0..=span.1, DefinitionKind::Declaration);
                super::tween::build_tween_definitions(body, definitions);
            }

            Construct::Derive { .. } | Construct::Priority { .. } => {
                let span = construct.span();
                definitions.insert(span.0..=span.1, DefinitionKind::Declaration);
            }

            Construct::MacroCall { .. } => {
                let span = construct.span();
                definitions.insert(span.0..=span.1, DefinitionKind::Declaration);
            }

            Construct::Macro {
                declaration,
                name,
                args,
                return_type,
                ..
            } => {
                let span_start = declaration.token.start();
                let span_end = return_type
                    .as_ref()
                    .map(|(arrow, ident)| {
                        ident
                            .as_ref()
                            .map(|i| i.token.end())
                            .unwrap_or(arrow.token.end())
                    })
                    .or_else(|| {
                        args.as_ref().map(|a| {
                            a.right
                                .as_ref()
                                .map(|r| r.token.end())
                                .unwrap_or(a.left.token.end())
                        })
                    })
                    .or_else(|| name.as_ref().map(|n| n.token.end()))
                    .unwrap_or(declaration.token.end());
                definitions.insert(span_start..=span_end, DefinitionKind::Declaration);
            }

            _ => (),
        }
    }
}
