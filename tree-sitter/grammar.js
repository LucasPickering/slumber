/**
 * @file Collection and template language for Slumber HTTP client
 * @author Lucas Pickering <lucas@lucaspickering.me>
 * @license MIT
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

export default grammar({
  name: "slumber",

  rules: {
    source_file: $ => repeat($.definition),
    definition: $ => choice(
      $.let_binding,
      $.profile,
      $.recipe,
    ),
    let_binding: $ => seq('let', $.identifier, '=', $.expression),
    identifier: $ => /[a-zA-Z0-9-_]+/,
    expression: $ => choice($.literal), // TODO more
    literal: $ => choice($.null_, $.bool), // TODO more
    null_: $ => 'null',
    bool: $ => choice('true', 'false'),

    profile: $ => seq('profile', $.identifier), // TODO block
    recipe: $ => seq('recipe', $.identifier), // TODO block
  }
});
