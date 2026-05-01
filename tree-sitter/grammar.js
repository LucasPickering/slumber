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

    // Config (static) rules
    definition: $ => choice($.local, $.profile, $.recipe),
    defer: $ => seq("@", $.deferred),
    local: $ => seq("local", $.identifier, "=", $.expression),
    identifier: _ => /[a-zA-Z0-9-_]+/,
    expression: $ => choice($.literal), // TODO more
    literal: $ => choice($.null_, $.bool), // TODO more
    null_: _ => "null",
    bool: _ => choice("true", "false"),
    map: $ => repeat(seq($.identifier, "=", $.expression)),
    block: $ => seq("{", repeat($.definition), $.expression, "}"),

    profile: $ => seq("profile", $.identifier), // TODO block
    recipe: $ => seq("recipe", $.identifier), // TODO block

    // Expression (dynamic) rules
    deferred: $ => choice($.expression, $.function),
    function: $ => seq("() => ", $.expression),
  },
});
