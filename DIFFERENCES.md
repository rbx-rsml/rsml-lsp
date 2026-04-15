# Differences: Old rsml-rust vs New rsml-lsp

## Genuinely Missing Syntactic Features

### 1. `@util` Declaration (entirely absent)
The old codebase has a full `@util` system with its own lexer (`utils_lexer.rs`) and parser (`utils_parser.rs`). It defines utility classes as `@util { name = "value"; ... }`. The new LSP has no `@util` token, no parser rule, and no AST construct for it. Users writing `@util` would get an "unknown declaration" error.

### 2. `ColorSkin` token not parsed as a datatype
The lexer defines `ColorSkin` (`skin:color:shade`), but the parser's `parse_datatype_part` in `datatype.rs` does not include `ColorSkin` in the match arms. It would be rejected as an unexpected token in value position, even though the other color tokens (`ColorTailwind`, `ColorBrick`, `ColorCss`, `ColorHex`) are all handled.

---

## Lexer Token Restructuring (not missing, just different)
These are architectural changes, not missing features — the new lexer combines prefix+identifier into single tokens:
- Old standalone `#`, `.`, `:`, `::`, `$`, `$!`, `&` tokens → New combined `NameSelector`, `TagSelectorOrEnumPart`, `StateSelectorOrEnumPart`, `PseudoSelector`, `TokenIdentifier`, `StaticTokenIdentifier`, `MacroArgIdentifier`
- Old `BoolTrue`/`BoolFalse` → New unified `Boolean(&str)`
- Old `Text` → New `Identifier(&str)` with payload
- Old `ScaleOrOpMod` ambiguity → New clean `NumberScale` + `OpMod` split
- Old standalone `px` token → New combined `NumberOffset` (`42px` as one token)

---

## Semantic Features (expected to be absent in LSP)
These are evaluation/runtime concerns the old compiler handled but the LSP intentionally omits:
- Macro expansion engine (token injection, recursion prevention, arity overloading)
- Built-in macros (`builtins.rsml`)
- Multi-file `@derive` loading from disk
- Value resolution to `rbx_types::Variant` (Color3, UDim, EnumItem, etc.)
- Color lookup tables (tailwind, brick, CSS, skin)
- Enum validation via `rbx_reflection_database`
- Arithmetic expression evaluation
- Static attribute (`$!`) scope-walking resolution
- `TreeNode` / `TreeNodeGroup` semantic tree building
- Tuple annotation type conversion (21 type-specific converters)
