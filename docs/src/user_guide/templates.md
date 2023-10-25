# Templates

Templates enable dynamic string construction. Slumber's template language is relatively simple, compared to complex HTML templating languages like Handlebars or Jinja. The goal is to be intuitive and unsurprising. It doesn't support complex features like for loops, conditionals, etc.

All string _values_ (i.e. _not_ keys) in a request collection are template strings, meaning they support templating. The syntax for templating a value into a string is double curly braces (`{{...}}`). The contents inside the braces tell Slumber how to retrieve the dynamic value.

TODO add some more content here

## API Reference

For more detailed configuration info, see the [API reference docs](../api/template_string.md) on template strings.
