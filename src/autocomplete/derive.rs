use std::path::{Path, PathBuf};

use rbx_rsml::lexer::{MultilineString, SpannedToken, Token};
use rbx_rsml::parser::{Construct, Delimited, Node};
use rbx_rsml::typechecker::{DefinitionKind, Definitions};
use rbx_rsml::typechecker::luaurc::Luaurc;

use super::normalize_path::NormalizePath;

fn resolve_derive_path(
    content: &str,
    current_path: &Path,
    luaurc: Option<&Luaurc>,
) -> PathBuf {
    let path = 'core: {
        let derived_path = PathBuf::from(content.trim()).normalize();

        let Some(luaurc) = luaurc else {
            break 'core derived_path;
        };

        let mut components = derived_path.components();

        let Some(component) = components.next() else {
            break 'core derived_path;
        };

        let component_str = component.as_os_str().to_string_lossy();

        if component_str.starts_with("@") {
            let alias = &component_str.as_ref()[1..];

            if let Some(alias_path) = luaurc.aliases.get(alias) {
                let mut resolved = PathBuf::from(alias_path);
                resolved.push(components);
                return resolved;
            } else {
                derived_path
            }
        } else {
            derived_path
        }
    };

    current_path.join("../").join(path)
}

pub fn build_derive_definitions(
    body: &Construct<'_>,
    current_path: &Path,
    luaurc: Option<&Luaurc>,
    definitions: &mut Definitions,
) {
    match body {
        Construct::Node {
            node:
                Node {
                    token:
                        SpannedToken(
                            span_start,
                            Token::StringSingle(content)
                            | Token::StringMulti(MultilineString { content, .. }),
                            span_end,
                        ),
                    ..
                },
        } => {
            let span = (*span_start, *span_end);
            let mut path = resolve_derive_path(content, current_path, luaurc);
            path.set_extension("rsml");

            match path.canonicalize() {
                Ok(canonicalized) => {
                    if &canonicalized != current_path {
                        definitions.insert(
                            span.0..=span.1,
                            DefinitionKind::Derive {
                                path: canonicalized,
                            },
                        );
                    }
                }
                Err(_) => {
                    definitions.insert(
                        span.0..=span.1,
                        DefinitionKind::Derive {
                            path: path.normalize(),
                        },
                    );
                }
            }
        }

        Construct::Table {
            body: Delimited { content, .. },
        } => {
            let Some(content) = content.as_ref() else {
                return;
            };

            for item in content {
                if matches!(
                    item,
                    Construct::Node {
                        node: Node {
                            token: SpannedToken(_, Token::SemiColon, _),
                            ..
                        },
                    }
                ) {
                    continue;
                }

                build_derive_definitions(item, current_path, luaurc, definitions);
            }
        }

        Construct::Node {
            node: Node {
                token: SpannedToken(_, Token::Comma, _),
                ..
            },
        } => (),

        _ => (),
    }
}
