# v0.2.1
- Now parses `@name` and `@priority` declarations.
- declarations can now prematurely terminate the parsing of a construct.
- Fixed issues when parsing assignments where they would erroneously consume construct terminators.

# v0.2.0
- Replaced LALRPOP with a hand-written recursive descent parser.

# v0.1.0
- Initial release, added a parser for syntax errors - written declaratively with LALRPOP.