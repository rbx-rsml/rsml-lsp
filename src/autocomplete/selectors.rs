use rbx_rsml::lexer::Token;
use rbx_rsml::parser::{Construct, Delimited};
use rbx_rsml::typechecker::{DefinitionKind, Definitions};

pub fn build_rule_body_definitions(
    body: &Option<Delimited<'_>>,
    definitions: &mut Definitions,
) {
    let Some(body) = body.as_ref() else { return };
    let Some(content) = body.content.as_ref() else {
        return;
    };

    // Look up the Scope from the body range to get type_definition.
    // The typechecker already inserted Scope definitions for each rule body.
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

    for construct in content {
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
                    .unwrap_or(middle.token.end());

                definitions.insert(
                    assign_start..=assign_end,
                    DefinitionKind::Assignment {
                        property_name: property_name.to_string(),
                        type_definition: current_classes.clone(),
                    },
                );

                let Some(right) = right else { continue };
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

                        definitions.insert(
                            name_range_start..=name_range_end,
                            DefinitionKind::EnumName,
                        );

                        if let Some(name_node) = name {
                            let enum_name = match name_node.token.value() {
                                Token::TagSelectorOrEnumPart(Some(name)) => name,
                                Token::StateSelectorOrEnumPart(Some(name)) => name,
                                _ => continue,
                            };

                            let variant_range_start = name_node.token.end();
                            let variant_range_end = variant
                                .as_ref()
                                .map(|node| node.token.end())
                                .unwrap_or(assign_end);

                            definitions.insert(
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

            Construct::Tween {
                body: Some(body), ..
            } => {
                let span = construct.span();
                definitions.insert(span.0..=span.1, DefinitionKind::Declaration);
                super::tween::build_tween_definitions(body, definitions);
            }

            Construct::Derive { .. }
            | Construct::Priority { .. }
            | Construct::Name { .. } => {
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
