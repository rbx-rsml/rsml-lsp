# v0.3.0
## Changes
- Added support for query selectors `@{ident}`.
- Macro's can now have a return context specified (`@macro Test -> Context {}`). Supported context's are `Construct`, `Selector` and `Assignment`.
- Added type hover hints for tokens.
- An error will be thrown when calling a macro which doesn't exist.
- An error will be thrown when a macro overload is invalid.
- An error will be thrown if a function's args are incorrect.
- An error will be thrown when referencing a token which doesn't exist.
- An errror will be thrown when a infinite recursion cycle is detected when calling a macro.
- Added typechecking for properties.

## Fixes
- Fixed types for rule selectors.
- Valid macro arguments (`&{ident}`) no longer throw type errors.
- Creating a pseudo selector for `StyleQuery` no longer throws a type error.
- Skin color codes no longer throw errors when used in property assignment.
- Minus signs in front of numeric datatypes no longer throw a parse error.

# v0.2.3
## Fixes
- Removed `rsml` files are now removed from the internal documents map.
- Removed/added workspaces are now removed/added from the internal workspaces map.

## Changes
- Errors are now shown for external cyclic dependencies.

# v0.2.2
## Fixes
- Fixed issue where diagnostics for multiline string errors didn't exist.
- Fixed issues where the `boolean`, `nil` and `rbxassetid://` datatypes didn't exist in the parser.

## Changes
- Added type-checking to derive statements - type errors will be thrown if:
  - non-string types are attempted to be derived.
  - the derived path can't be resolved to a file.
  - the current file is attempted to be derived (external cyclic dependency detection coming in a later update).

- Added type-checking to selectors - type errors will be thrown if:
  - a selector selects more than one class.
  - a selector part selects an non-existant class or state.
  - a selector part selects a class which can't be used as a pseudo selector.
  - a selector part appears after a pseudo selector.

- Added type hover hints to selectors and derives.

- Added go to definition support for derives.

# v0.2.1
- Now parses `@name` and `@priority` declarations.
- declarations can now prematurely terminate the parsing of a construct.
- Fixed issues when parsing assignments where they would erroneously consume construct terminators.

# v0.2.0
- Replaced LALRPOP with a hand-written recursive descent parser.

# v0.1.0
- Initial release, added a parser for syntax errors - written declaratively with LALRPOP.
