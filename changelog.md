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
  - a selector part appears after a psuedo selector.

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