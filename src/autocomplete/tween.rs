use rbx_rsml::lexer::{SpannedToken, Token};
use rbx_rsml::parser::{Construct, Delimited, Node};
use rbx_rsml::typechecker::{DefinitionKind, Definitions};

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

fn register_enum_arg_definitions(
    arg: &Construct,
    enum_name: &str,
    slot_end: usize,
    definitions: &mut Definitions,
) {
    match arg {
        Construct::Enum {
            keyword,
            name,
            variant,
        } => {
            let name_range_start = keyword.token.end();
            let name_range_end = name
                .as_ref()
                .map(|node| node.token.end())
                .unwrap_or(slot_end);

            definitions.insert(
                name_range_start..=name_range_end,
                DefinitionKind::FilteredEnumName {
                    enum_name: enum_name.to_string(),
                },
            );

            if let Some(name_node) = name {
                let has_name = matches!(
                    name_node.token.value(),
                    Token::TagSelectorOrEnumPart(Some(_))
                        | Token::StateSelectorOrEnumPart(Some(_))
                );

                if has_name {
                    let variant_range_start = name_node.token.end();
                    let variant_range_end = variant
                        .as_ref()
                        .map(|node| node.token.end())
                        .unwrap_or(slot_end);

                    definitions.insert(
                        variant_range_start..=variant_range_end,
                        DefinitionKind::EnumVariant {
                            enum_name: enum_name.to_string(),
                        },
                    );
                }
            }
        }

        _ => {
            let arg_span = arg.span();
            definitions.insert(
                arg_span.0..=arg_span.1,
                DefinitionKind::EnumVariant {
                    enum_name: enum_name.to_string(),
                },
            );
        }
    }
}

pub fn build_tween_definitions(body: &Construct<'_>, definitions: &mut Definitions) {
    let Construct::Table {
        body:
            Delimited {
                content: Some(items),
                ..
            },
    } = body
    else {
        return;
    };

    let args: Vec<&Construct<'_>> = items.iter().filter(|item| !is_comma(item)).collect();

    if args.is_empty() {
        return;
    }

    let tuple_end = body.span().1;

    if let Some(arg) = args.get(1) {
        let slot_end = args.get(2).map(|a| a.span().0).unwrap_or(tuple_end);
        register_enum_arg_definitions(arg, "EasingStyle", slot_end, definitions);
    }

    if let Some(arg) = args.get(2) {
        register_enum_arg_definitions(arg, "EasingDirection", tuple_end, definitions);
    }
}
