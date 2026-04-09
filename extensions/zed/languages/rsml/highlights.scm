; Fallback identifier (must be first so more specific rules below take precedence)
(identifier) @variable

; Comments
(comment) @comment

; Strings
(single_quote_string) @string
(double_quote_string) @string
(block_string) @string
(string_content) @string

; Numbers
(number_ambiguous) @number
(number_percent) @number
(number_offset) @number

; Colors
(color_hex) @number
(color_code_tailwind) @string
(color_code_skin) @string
(color_code_brick) @string
(color_code_css) @string

; Enums
(enum_full) @tag
(enum_shorthand) @tag

; Keywords / Declarations
"@macro" @keyword
"@priority" @keyword
"@name" @keyword
"@derive" @keyword

; Macro calls
(macro_call
  annotation: (_) @function)

; Selectors
(class_selector
  (identifier) @tag)
(name_selector) @tag
(tag_selector) @tag
(state_selector) @tag
(pseudo_selector) @tag

; References / Variables
(token) @variable.special
(static_token) @variable.special
(static_argument) @variable.special

; Properties
(property_assignment
  (identifier) @property)

; Operators
(operator) @operator
(equals) @operator

; Punctuation
(comma) @punctuation.delimiter
(semi_colon) @punctuation.delimiter
(colon) @punctuation.delimiter
(scope_open) @punctuation.bracket
(scope_close) @punctuation.bracket
(tuple_open) @punctuation.bracket
(tuple_close) @punctuation.bracket

; Tuple annotation
(tuple
  annotation: (identifier) @function)

; URLs
(rbx_asset) @string.special
(rbx_content) @string.special
