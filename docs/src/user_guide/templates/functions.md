# Functions

Template functions enable you to build dynamic templates in a way that's intuitive and composable. Slumber templates behave as a simple functional language: functions take arguments and evaluate to values. All functions are provided by Slumber; **there is no way to define your own functions.**

**For the list of available functions, see [Template Functions](../../api/template_functions.md).**

## Arguments

Slumber functions take two types of arguments:

- Positional arguments are specified in a specific and are always required
- Keyword arguments as passed in the form `key=value`, can be specified in any order (provided all positional arguments are passed first), and are always optional

In the function signatures listed below, keyword arguments are specified with a `?` while positional arguments are not.

For example, this function takes 2 required positional arguments and 2 optional keyword arguments:

```typescript
func(a: string, b: boolean, c?: string, d?: bytes): string
```

Given this function signature, the following are all valid calls:

```python
func("hello", true);
func("hello", true, "world");
func("hello", true, c="world");
func("hello", true, d=b"bytes");
func("hello", true, c="world", d=b"bytes");
func("hello", true, d=b"bytes", c="world"); # Keyword args can be reordered
```

The follow calls are **not valid**:

```python
# WARNING: Invalid code!!
func("hello") # Required arguments omitted
func(c="world", "hello", true) # Keyword argument before positional
func("hello", true, c="world", c="world") # Keyword argument given twice
func("hello", true, "world") # Optional arguments must be given by name
```

### Defaults

If a keyword argument is omitted, it will be replaced by a default value. In most cases, the default will be based on the type of the argument:

- `boolean`: `false`
- `number`: `0`
- `string`: `""`
- `bytes`: `b""`
- `array`: `[]`
- `value`: `null`

If the default varies from this list, it will be specified in the `Parameters` section of the function's docs.

## Pipe Operator

It's common to take the output of one function and pass it to another. This is especially useful for filter-esque functions like [`jsonpath`](../../api/template_functions.md#jsonpath) and [`trim`](../../api/template_functions.md#trim) that modify incoming input. Here's an example using [`command`](../../api/template_functions.md#command) and [`trim`](../../api/template_functions.md#trim)

```python
# Command output often includes a trailing newline that we want to trim away
trim(command(["echo", "hello"]))
```

This works, but it's a bit backward: we run the `command`, _then_ `trim` it. To make these types of composed operations easier to read and write, Slumber supports the pipe operator `|`. The left-hand side of the operator can be any expression, but is typically a function call. The right-hand side **must be a function call**. The left-hand side is evaluated, then the result is passed as the **last** argument to the right-hand side. We can rewrite the same expression from above with the pipe:

```python
# Equivalent to the above expression
command(["echo", "hello"]) | trim()
```

This is equivalent, but easier to read because the lexical ordering of calls matches the evaluation order.

> Unlike other template languages such as Jinja and Tera, the right-hand side of a pipe **must include parentheses**, even if they argument list is empty. Additionally, other languages have a distinction between "functions" and "filters", and only filters can be used on the right-hand side of a pipe operation. This distinction does **not** exist in Slumber; any function can be used on the right-hand side of a pipe, as long as it takes at least one positional argument.

Remember: the piped value is passed as the **last** positional argument to the right-hand side. That means it will be inserted after other positional arguments but before any keyword arguments. Here's another example, using [`response`](../../api/template_functions.md#response) and [`jsonpath`](../../api/template_functions.md#jsonpath).

```python
response('login') | jsonpath("$.token", mode="single")
# is equivalent to
jsonpath("$.token", response('login'), mode="single")
```
