use rbx_reflection::{DataType, PropertyDescriptor, PropertyKind, Scriptability};
use rbx_rsml::datatype::palette;
use rbx_types::VariantType;
use tower_lsp::lsp_types::{Command, CompletionItem, CompletionItemKind, InsertTextFormat};

/// Asks the client to re-fire completion after the edit is applied. Editors
/// don't auto-retrigger on accept even if the inserted text ends in a trigger
/// character, so items that open a further menu (`tw:` stub, a Tailwind family
/// that has shades, …) need to request it explicitly. `editor.action.triggerSuggest`
/// is the VS Code command id; Zed recognises the same string.
fn retrigger_command() -> Command {
    Command {
        title: "Trigger Suggest".to_string(),
        command: "editor.action.triggerSuggest".to_string(),
        arguments: None,
    }
}

/// Classifies what the user has typed immediately before the cursor within a
/// property assignment's RHS, so `get_value_completions` can choose between
/// bool literals, constructors, enum variants, and the three-tier color flow.
pub enum ValuePrefix<'a> {
    Fresh,
    EnumShorthand,
    TailwindFamily { typed: &'a str },
    TailwindShade { family: &'a str },
    SkinFamily { typed: &'a str },
    SkinShade { family: &'a str },
    BrickFamily { typed: &'a str },
    CssFamily { typed: &'a str },
    InsideConstructor,
}

/// Walks backward from `offset` across value-identifier bytes to find the raw
/// slice the user is mid-typing. The lexer can't tokenise a partial color
/// literal like `tw:` or `tw:red:`, so we reconstruct it from source directly.
pub fn scan_value_prefix(source: &str, offset: usize) -> ValuePrefix<'_> {
    let bytes = source.as_bytes();
    let capped_offset = offset.min(bytes.len());

    let mut start = capped_offset;

    while start > 0 {
        let byte = bytes[start - 1];
        let is_word = byte.is_ascii_alphanumeric() || byte == b':' || byte == b'_';

        if !is_word {
            break;
        }

        start -= 1;
    }

    let slice = &source[start..capped_offset];

    if capped_offset > 0 && bytes[capped_offset - 1] == b'(' {
        return ValuePrefix::InsideConstructor;
    }

    match slice {
        "" => return ValuePrefix::Fresh,
        _ => (),
    }

    if let Some(rest) = slice.strip_prefix("tw:") {
        return classify_two_level(rest, |family| ValuePrefix::TailwindShade { family }, |typed| {
            ValuePrefix::TailwindFamily { typed }
        });
    }

    if let Some(rest) = slice.strip_prefix("skin:") {
        return classify_two_level(rest, |family| ValuePrefix::SkinShade { family }, |typed| {
            ValuePrefix::SkinFamily { typed }
        });
    }

    if let Some(rest) = slice.strip_prefix("bc:") {
        return ValuePrefix::BrickFamily { typed: rest };
    }

    if let Some(rest) = slice.strip_prefix("css:") {
        return ValuePrefix::CssFamily { typed: rest };
    }

    // Bare `:` (or `:Partial`) inside an assignment RHS means enum shorthand,
    // since any color prefix would have matched above.
    if slice.starts_with(':') {
        return ValuePrefix::EnumShorthand;
    }

    ValuePrefix::Fresh
}

fn classify_two_level<'a>(
    rest: &'a str,
    shade: impl FnOnce(&'a str) -> ValuePrefix<'a>,
    family: impl FnOnce(&'a str) -> ValuePrefix<'a>,
) -> ValuePrefix<'a> {
    match rest.split_once(':') {
        Some((family_name, _shade_typed)) => shade(family_name),
        None => family(rest),
    }
}

pub fn get_value_completions(
    source: &str,
    offset: usize,
    class_names: &[String],
    property_name: &str,
) -> Vec<CompletionItem> {
    let prefix = scan_value_prefix(source, offset);

    match prefix {
        ValuePrefix::InsideConstructor => vec![],

        ValuePrefix::EnumShorthand => enum_shorthand_items(class_names, property_name),

        ValuePrefix::TailwindFamily { .. } => color_items(palette::tailwind_families(), "tw", true),

        ValuePrefix::TailwindShade { family } => shade_items(palette::tailwind_shades(family), "tw", family),

        ValuePrefix::SkinFamily { .. } => color_items(palette::skin_families(), "skin", true),

        ValuePrefix::SkinShade { family } => shade_items(palette::skin_shades(family), "skin", family),

        ValuePrefix::BrickFamily { .. } => color_items(palette::brick_names(), "bc", false),

        ValuePrefix::CssFamily { .. } => color_items(palette::css_names(), "css", false),

        ValuePrefix::Fresh => fresh_items(class_names, property_name),
    }
}

fn color_items(
    names: &[&'static str],
    prefix: &str,
    has_next_level: bool,
) -> Vec<CompletionItem> {
    names
        .iter()
        .map(|name| {
            // Append a trailing `:` when the palette has a further level
            // (Tailwind/Skin → shades). The retrigger command then opens
            // the shade menu immediately; without the `:` the re-scan
            // would classify the buffer as `*Family` again and loop.
            let (insert_text, command) = if has_next_level {
                (Some(format!("{}:", name)), Some(retrigger_command()))
            } else {
                (None, None)
            };

            CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::COLOR),
                detail: Some(format!("{}:{}", prefix, name)),
                insert_text,
                command,
                ..CompletionItem::default()
            }
        })
        .collect()
}

fn shade_items(shades: &[&'static str], prefix: &str, family: &str) -> Vec<CompletionItem> {
    shades
        .iter()
        .map(|shade| CompletionItem {
            label: shade.to_string(),
            kind: Some(CompletionItemKind::COLOR),
            detail: Some(format!("{}:{}:{}", prefix, family, shade)),
            ..CompletionItem::default()
        })
        .collect()
}

fn fresh_items(class_names: &[String], property_name: &str) -> Vec<CompletionItem> {
    let Some(descriptor) = lookup_descriptor(class_names, property_name) else {
        return vec![];
    };

    match &descriptor.data_type {
        DataType::Enum(enum_name) => {
            let mut items = enum_variant_items(enum_name.as_ref());
            items.extend(enum_variant_longform_items(enum_name.as_ref()));
            items
        }

        DataType::Value(variant_type) => value_type_items(*variant_type),

        _ => vec![],
    }
}

pub fn property_enum_name(class_names: &[String], property_name: &str) -> Option<String> {
    let descriptor = lookup_descriptor(class_names, property_name)?;

    if let DataType::Enum(enum_name) = &descriptor.data_type {
        Some(enum_name.to_string())
    } else {
        None
    }
}

fn lookup_descriptor(
    class_names: &[String],
    property_name: &str,
) -> Option<&'static PropertyDescriptor<'static>> {
    let Ok(db) = rbx_reflection_database::get() else {
        return None;
    };

    for class_name in class_names {
        let Some(class_desc) = db.classes.get(class_name.as_str()) else {
            continue;
        };

        for ancestor in db.superclasses_iter(class_desc) {
            let Some(prop_desc) = ancestor.properties.get(property_name) else {
                continue;
            };

            if matches!(prop_desc.kind, PropertyKind::Alias { .. }) {
                continue;
            }

            if matches!(prop_desc.scriptability, Scriptability::None) {
                continue;
            }

            return Some(prop_desc);
        }
    }

    None
}

fn enum_variant_items(enum_name: &str) -> Vec<CompletionItem> {
    enum_variant_items_inner(enum_name, true)
}

fn enum_shorthand_items(class_names: &[String], property_name: &str) -> Vec<CompletionItem> {
    let Some(descriptor) = lookup_descriptor(class_names, property_name) else {
        return vec![];
    };

    let DataType::Enum(enum_name) = &descriptor.data_type else {
        return vec![];
    };

    enum_variant_items_inner(enum_name.as_ref(), false)
}

fn enum_variant_longform_items(enum_name: &str) -> Vec<CompletionItem> {
    let Ok(db) = rbx_reflection_database::get() else {
        return vec![];
    };

    let Some(enum_desc) = db.enums.get(enum_name) else {
        return vec![];
    };

    enum_desc
        .items
        .keys()
        .map(|variant| {
            let literal = format!("Enum.{}.{}", enum_name, variant);

            CompletionItem {
                label: literal.clone(),
                kind: Some(CompletionItemKind::ENUM_MEMBER),
                insert_text: Some(literal.clone()),
                detail: Some(literal),
                ..CompletionItem::default()
            }
        })
        .collect()
}

fn enum_variant_items_inner(enum_name: &str, prepend_colon: bool) -> Vec<CompletionItem> {
    let Ok(db) = rbx_reflection_database::get() else {
        return vec![];
    };

    let Some(enum_desc) = db.enums.get(enum_name) else {
        return vec![];
    };

    enum_desc
        .items
        .keys()
        .map(|variant| {
            let (label, insert_text) = if prepend_colon {
                let literal = format!(":{}", variant);
                (literal.clone(), Some(literal))
            } else {
                (variant.to_string(), None)
            };

            CompletionItem {
                label,
                kind: Some(CompletionItemKind::ENUM_MEMBER),
                insert_text,
                detail: Some(format!("Enum.{}.{}", enum_name, variant)),
                ..CompletionItem::default()
            }
        })
        .collect()
}

fn value_type_items(variant_type: VariantType) -> Vec<CompletionItem> {
    match variant_type {
        VariantType::Bool => vec![bool_item("true"), bool_item("false")],

        VariantType::Color3 | VariantType::Color3uint8 => color3_items(),

        VariantType::UDim => constructor_items(&["udim"]),

        VariantType::UDim2 => constructor_items(&["udim2"]),

        VariantType::Vector2 => constructor_items(&["vec2"]),

        VariantType::Vector2int16 => constructor_items(&["vec2i16"]),

        VariantType::Vector3 => constructor_items(&["vec3"]),

        VariantType::Vector3int16 => constructor_items(&["vec3i16"]),

        VariantType::CFrame => constructor_items(&["cframe"]),

        VariantType::Rect => constructor_items(&["rect"]),

        VariantType::NumberRange => constructor_items(&["numrange"]),

        VariantType::NumberSequence => constructor_items(&["numseq"]),

        VariantType::ColorSequence => constructor_items(&["colorseq"]),

        VariantType::Font => constructor_items(&["font"]),

        VariantType::Content | VariantType::ContentId => constructor_items(&["content"]),

        VariantType::BrickColor => constructor_items(&["brickcolor"]),

        _ => vec![],
    }
}

fn bool_item(name: &str) -> CompletionItem {
    CompletionItem {
        label: name.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        ..CompletionItem::default()
    }
}

fn constructor_arg_defaults(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "udim" | "vec2" | "vec2i16" | "numrange" => Some(&["0", "0"]),
        "vec3" | "vec3i16" | "color3" | "rgb" | "oklab" | "oklch" | "cframe" => {
            Some(&["0", "0", "0"])
        }
        "udim2" | "rect" => Some(&["0", "0", "0", "0"]),
        "brickcolor" | "content" => Some(&["\"\""]),
        "numseq" | "floor" | "ceil" | "round" | "abs" => Some(&["0"]),
        "colorseq" => Some(&["color3(0, 0, 0)"]),
        "font" => Some(&["\"SourceSansPro\""]),
        "lerp" => Some(&["0", "0", "0"]),
        _ => None,
    }
}

fn constructor_items(names: &[&str]) -> Vec<CompletionItem> {
    names
        .iter()
        .map(|name| {
            let insert_text = match constructor_arg_defaults(name) {
                Some(defaults) => {
                    let args = defaults
                        .iter()
                        .enumerate()
                        .map(|(index, default)| format!("${{{}:{}}}", index + 1, default))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{} ({})$0", name, args)
                }

                None => format!("{} ($0)", name),
            };

            CompletionItem {
                label: format!("{} (…)", name),
                kind: Some(CompletionItemKind::FUNCTION),
                insert_text: Some(insert_text),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                detail: Some(format!("{} (...)", name)),
                ..CompletionItem::default()
            }
        })
        .collect()
}

fn color3_items() -> Vec<CompletionItem> {
    let mut items = constructor_items(&["color3", "rgb"]);

    for prefix in ["tw:", "skin:", "bc:", "css:"] {
        items.push(CompletionItem {
            label: prefix.to_string(),
            kind: Some(CompletionItemKind::COLOR),
            detail: Some(format!("{}…", prefix)),
            command: Some(retrigger_command()),
            ..CompletionItem::default()
        });
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }

    #[test]
    fn scan_fresh_after_equals() {
        let prefix = scan_value_prefix("Frame { Size = ", 15);
        assert!(matches!(prefix, ValuePrefix::Fresh));
    }

    #[test]
    fn scan_tailwind_family_bare() {
        let prefix = scan_value_prefix("= tw:", 5);
        assert!(matches!(prefix, ValuePrefix::TailwindFamily { typed: "" }));
    }

    #[test]
    fn scan_tailwind_family_partial_name() {
        let prefix = scan_value_prefix("= tw:re", 7);
        assert!(matches!(prefix, ValuePrefix::TailwindFamily { typed: "re" }));
    }

    #[test]
    fn scan_tailwind_shade() {
        let prefix = scan_value_prefix("= tw:red:", 9);
        assert!(matches!(prefix, ValuePrefix::TailwindShade { family: "red" }));
    }

    #[test]
    fn scan_css() {
        let prefix = scan_value_prefix("= css:", 6);
        assert!(matches!(prefix, ValuePrefix::CssFamily { typed: "" }));
    }

    #[test]
    fn scan_brick() {
        let prefix = scan_value_prefix("= bc:", 5);
        assert!(matches!(prefix, ValuePrefix::BrickFamily { typed: "" }));
    }

    #[test]
    fn scan_inside_constructor() {
        let prefix = scan_value_prefix("= udim2(", 8);
        assert!(matches!(prefix, ValuePrefix::InsideConstructor));
    }

    #[test]
    fn value_completions_bool() {
        let items = get_value_completions("= ", 2, &["Frame".to_string()], "Visible");
        let got = labels(&items);
        assert!(got.contains(&"true"));
        assert!(got.contains(&"false"));
    }

    #[test]
    fn value_completions_udim2() {
        let items = get_value_completions("= ", 2, &["Frame".to_string()], "Size");
        let got = labels(&items);
        assert!(got.iter().any(|label| label.starts_with("udim2")));
    }

    #[test]
    fn constructor_insert_text_prefills_numeric_defaults() {
        let items = get_value_completions("= ", 2, &["Frame".to_string()], "Size");
        let udim2 = items
            .iter()
            .find(|item| item.label.starts_with("udim2"))
            .expect("udim2 completion missing");

        let insert_text = udim2.insert_text.as_deref().unwrap_or("");
        assert_eq!(insert_text, "udim2 (${1:0}, ${2:0}, ${3:0}, ${4:0})$0");
    }

    #[test]
    fn constructor_defaults_cover_every_tuple_annotation() {
        // Mirror of TUPLE_ANNOTATIONS keys at
        // rsml-rust-rewrite/src/datatype/tuple/tuple_annotations/mod.rs.
        // The `tuple` module is crate-internal in rbx_rsml, so we can't
        // import the map directly — keep this list in sync by hand.
        const EXPECTED: &[&str] = &[
            "udim", "udim2", "rect", "vec2", "vec2i16", "vec3", "vec3i16",
            "cframe", "color3", "rgb", "oklab", "oklch", "brickcolor",
            "colorseq", "numseq", "numrange", "font", "content", "lerp",
            "floor", "ceil", "round", "abs",
        ];

        for name in EXPECTED {
            assert!(
                constructor_arg_defaults(name).is_some(),
                "constructor_arg_defaults missing default for `{}`",
                name
            );
        }
    }

    #[test]
    fn constructor_insert_text_prefills_string_defaults() {
        let items = get_value_completions(
            "= ",
            2,
            &["Frame".to_string()],
            "BrickColor",
        );
        let brickcolor = items
            .iter()
            .find(|item| item.label.starts_with("brickcolor"));

        if let Some(item) = brickcolor {
            let insert_text = item.insert_text.as_deref().unwrap_or("");
            assert_eq!(insert_text, "brickcolor (${1:\"\"})$0");
        }
    }

    #[test]
    fn value_completions_color3_fresh_has_prefixes() {
        let items = get_value_completions(
            "= ",
            2,
            &["Frame".to_string()],
            "BackgroundColor3",
        );
        let got = labels(&items);
        assert!(got.contains(&"tw:"));
        assert!(got.contains(&"skin:"));
        assert!(got.contains(&"bc:"));
        assert!(got.contains(&"css:"));
        assert!(got.iter().any(|label| label.starts_with("color3")));
        assert!(got.iter().any(|label| label.starts_with("rgb")));
    }

    #[test]
    fn tailwind_family_list_has_red() {
        let items = get_value_completions(
            "= tw:",
            5,
            &["Frame".to_string()],
            "BackgroundColor3",
        );
        assert!(labels(&items).contains(&"red"));
    }

    #[test]
    fn tailwind_shade_list_has_500() {
        let items = get_value_completions(
            "= tw:red:",
            9,
            &["Frame".to_string()],
            "BackgroundColor3",
        );
        let got = labels(&items);
        assert!(got.contains(&"500"));
        assert!(!got.contains(&"red"));
    }

    #[test]
    fn tailwind_family_item_inserts_trailing_colon_and_retriggers() {
        let items = get_value_completions(
            "= tw:",
            5,
            &["Frame".to_string()],
            "BackgroundColor3",
        );
        let red = items
            .iter()
            .find(|item| item.label == "red")
            .expect("red family missing");
        assert_eq!(red.insert_text.as_deref(), Some("red:"));
        assert_eq!(
            red.command.as_ref().map(|c| c.command.as_str()),
            Some("editor.action.triggerSuggest")
        );
    }

    #[test]
    fn css_name_item_does_not_retrigger() {
        let items = get_value_completions(
            "= css:",
            6,
            &["Frame".to_string()],
            "BackgroundColor3",
        );
        let any = items.first().expect("css items missing");
        assert!(any.insert_text.is_none());
        assert!(any.command.is_none());
    }

    #[test]
    fn color3_prefix_stubs_retrigger() {
        let items = get_value_completions(
            "= ",
            2,
            &["Frame".to_string()],
            "BackgroundColor3",
        );
        for prefix in ["tw:", "skin:", "bc:", "css:"] {
            let item = items
                .iter()
                .find(|item| item.label == prefix)
                .expect("prefix stub missing");
            assert_eq!(
                item.command.as_ref().map(|c| c.command.as_str()),
                Some("editor.action.triggerSuggest"),
                "{} stub should retrigger",
                prefix
            );
        }
    }

    #[test]
    fn tw_amber_trailing_colon_returns_shades() {
        let items = get_value_completions(
            "= tw:amber:",
            11,
            &["Frame".to_string()],
            "BackgroundColor3",
        );
        let got = labels(&items);
        assert!(got.contains(&"500"));
        assert!(!got.contains(&"amber"));
    }

    #[test]
    fn value_completions_enum_shorthand_items_have_colon_prefix() {
        let items = get_value_completions(
            "= ",
            2,
            &["Frame".to_string()],
            "AutomaticSize",
        );
        let shorthand: Vec<_> = items
            .iter()
            .filter(|item| !item.label.starts_with("Enum."))
            .collect();
        assert!(!shorthand.is_empty());
        assert!(shorthand.iter().all(|item| item.label.starts_with(':')));
        assert!(
            shorthand
                .iter()
                .all(|item| item.insert_text.as_deref().unwrap_or("").starts_with(':'))
        );
    }

    #[test]
    fn value_completions_enum_includes_longform_items() {
        let items = get_value_completions(
            "= ",
            2,
            &["Frame".to_string()],
            "AutomaticSize",
        );
        let got = labels(&items);
        assert!(got.contains(&"Enum.AutomaticSize.X"));
        assert!(got.contains(&"Enum.AutomaticSize.Y"));
        assert!(got.contains(&":X"));
    }

    #[test]
    fn value_completions_enum_longform_insert_matches_label() {
        let items = get_value_completions(
            "= ",
            2,
            &["Frame".to_string()],
            "AutomaticSize",
        );
        let longform: Vec<_> = items
            .iter()
            .filter(|item| item.label.starts_with("Enum."))
            .collect();
        assert!(!longform.is_empty());
        assert!(
            longform
                .iter()
                .all(|item| item.insert_text.as_deref() == Some(item.label.as_str()))
        );
    }

    #[test]
    fn scan_enum_shorthand_bare_colon() {
        let prefix = scan_value_prefix("= :", 3);
        assert!(matches!(prefix, ValuePrefix::EnumShorthand));
    }

    #[test]
    fn value_completions_post_colon_property_filtered() {
        let items = get_value_completions(
            "= :",
            3,
            &["Frame".to_string()],
            "AutomaticSize",
        );
        assert!(!items.is_empty());
        assert!(items.iter().all(|item| item.insert_text.is_none()));
        assert!(labels(&items).contains(&"X"));
    }

    #[test]
    fn value_completions_post_colon_non_enum_returns_empty() {
        let items = get_value_completions(
            "= :",
            3,
            &["Frame".to_string()],
            "BackgroundColor3",
        );
        assert!(items.is_empty());
    }

    #[test]
    fn value_completions_number_is_empty() {
        let items = get_value_completions(
            "= ",
            2,
            &["Frame".to_string()],
            "BorderSizePixel",
        );
        assert!(items.is_empty());
    }
}
