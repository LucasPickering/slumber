# Values

Templates contain expressions, and expressions evaluate to values. Template values are basically JSON values, with the addition of one more type: `bytes`. Here's the full list:

- `null`
- `boolean`: `true` or `false`
- `float`
  - Uses [f64](https://doc.rust-lang.org/std/primitive.f64.html) internally. See docs for information on min/max values.
- `integer` (signed integer)
  - Uses [i64](https://doc.rust-lang.org/std/primitive.i64.html) internally. See docs for information on min/max values.
- `string`: `"hello!"`
- `bytes`: `b"hello!"`
- `array`: `[1, false, "hello!"]`
- `object`: Currently not representable as a literal. Objects can be constructed in two ways:
  - Output of the [`jsonpath`](../../api/template_functions.md#jsonpath) function
  - From a `json` request body

## `bytes` vs `string`

A `string` is technically a subset of `bytes`: any sequence of valid UTF-8 bytes can be a `string`. Many functions return a `bytes` value because Slumber doesn't know if the value is valid UTF-8 or not. You may wonder: what do I do with this? How do I turn it into a string? You don't have to! There are three scenarios in which `bytes` can be used:

- You have a `bytes` but need a `string`. The bytes are valid UTF-8. Slumber will automatically convert it to a `string` when necessary.
- You have a `bytes` but need a `string`. The bytes are **not** valid UTF-8. Slumber will attempt to convert it to a `string` and fail, returning an error during request render.
- You have a `bytes` and need a `bytes` (e.g. for a request body, which doesn't need to be valid UTF-8). Easy!

So the short answer is: if you see a function return `bytes`, you can generally pretend it says `string`. The types are distinct to acknowledge the fact that the `bytes` _may_ not be valid UTF-8, and therefore may trigger errors while rendering a request.
