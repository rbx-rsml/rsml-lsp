use rbx_rsml::lexer::Token;
use rbx_rsml::parser::{Construct, Delimited};
use rbx_rsml::typechecker::{DefinitionKind, Definitions};

pub fn walk_construct(construct: &Construct, definitions: &mut Definitions) {
    match construct {
        Construct::Node { node } => {
            let (name, is_static) = match node.token.value() {
                Token::TokenIdentifier(name) => (*name, false),
                Token::StaticTokenIdentifier(name) => (*name, true),
                _ => return,
            };
            let (start, end) = node.token.span();
            definitions.insert(
                start..=end,
                DefinitionKind::Token {
                    name: name.to_string(),
                    is_static,
                },
            );
        }

        Construct::MathOperation { left, right, .. } => {
            walk_construct(left, definitions);
            if let Some(right) = right {
                walk_construct(right, definitions);
            }
        }

        Construct::UnaryMinus { operand, .. } => {
            walk_construct(operand, definitions);
        }

        Construct::Table { body } => {
            walk_delimited(body, definitions);
        }

        Construct::AnnotatedTable {
            body: Some(body), ..
        } => {
            walk_delimited(body, definitions);
        }

        Construct::MacroCall {
            body: Some(body), ..
        } => {
            walk_delimited(body, definitions);
        }

        _ => (),
    }
}

fn walk_delimited(delim: &Delimited, definitions: &mut Definitions) {
    let Some(content) = delim.content.as_ref() else {
        return;
    };
    for construct in content {
        walk_construct(construct, definitions);
    }
}
