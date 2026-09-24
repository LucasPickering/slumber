// TODO doc comments on everything

// TODO is it possible to statically type the available profile fields?
// TODO rename the primtives - they're not templates
type TemplatePrimitive = null | boolean | string | number;
type Complex<T> =
  | T
  | Array<Complex<T>>
  | { [key: string]: Complex<T> };
type TemplateValue = Complex<TemplatePrimitive>;
// TODO rename: it's not necessarily a string template
// TODO dedupe this stuff
type Template<T = TemplatePrimitive> =
  | T
  // TODO explain why we don't return Template<T>
  | ((profile: { [field: string]: unknown }) => Complex<T>)
  // TODO explain
  | Array<Template<T>>
  | { [key: string]: Template<T> };
type BinaryTemplate = Template<TemplatePrimitive | Bytes>;
type StreamTemplate = Template<TemplatePrimitive | Bytes | Stream>;

type Profiles = { [id: string]: Profile };

interface Profile {
  name?: string;
  default?: boolean;
  // Only static values are supported here; no templates
  data?: { [field: string]: TemplateValue };
}

type Recipes = { [id: string]: Folder | Recipe };

export interface Folder {
  name?: string;
  recipes: Recipes;
}

// TODO different typing for string vs binary
export interface Recipe {
  name?: string;
  persist?: boolean;
  method: HttpMethod;
  url: Template;
  query?: { [header: string]: Template | Template[] };
  headers?: { [header: string]: BinaryTemplate };
  body?:
    | StreamTemplate
    | { type: "formUrlencoded"; data: { [field: string]: Template } }
    | { type: "formMultipart"; data: { [field: string]: StreamTemplate } }
    | { type: "json"; data: Template };
  authentication?:
    | { type: "basic"; username: Template; password?: Template }
    | { type: "bearer"; token: Template };
}

type HttpMethod =
  | "CONNECT"
  | "DELETE"
  | "GET"
  | "HEAD"
  | "OPTIONS"
  | "PATCH"
  | "POST"
  | "PUT"
  | "TRACE";

// TODO update comment
// Some functions have a `decode` kwargs that controls whether the output is
// returned as bytes or not. By default we always decode it. They're overloaded
// so specifying decode:false will change the output type to Blob.

export class Bytes {
  $type: "bytes";
}
export class Stream {
  $type: "stream";
}

export function command(
  command: string[],
  kwargs?: CommandKwargs & { output?: "string" },
): string;
export function command(
  command: string[],
  kwargs: CommandKwargs & { output: "bytes" },
): Bytes;
export function command(
  command: string[],
  kwargs: CommandKwargs & { output: "stream" },
): Stream;
interface CommandKwargs {
  cwd?: string;
  output?: "string" | "bytes" | "stream";
  stdin?: string | Bytes;
}

export function file(path: string, kwargs?: { output?: "string" }): string;
export function file(path: string, kwargs: { output: "bytes" }): Bytes;
export function file(path: string, kwargs: { output: "stream" }): Stream;

export function prompt(
  kwargs?: { message?: string; default?: string; sensitive?: boolean },
): string;

export function response<T>(
  recipeId: string,
  kwargs?: ResponseKwargs & { output?: "todo" },
): T;
// When not decoding, the response can bytes or UTF-8 string
export function response(
  recipeId: string,
  kwargs: ResponseKwargs & { output: "string" },
): string;
export function response(
  recipeId: string,
  kwargs: ResponseKwargs & { output: "bytes" },
): Bytes;
interface ResponseKwargs {
  trigger?: "never" | "noHistory" | "always" | string;
  output?: "todo" | "string" | "bytes";
}

export function responseHeader(
  recipeId: string,
  header: string,
  kwargs?: ResponseKwargs & { output?: "string" },
): string;
export function responseHeader(
  recipeId: string,
  header: string,
  kwargs?: ResponseKwargs & { output: "bytes" },
): Bytes;

// Generics allows for type restriction when doing operations on the output
export function select<T extends TemplateValue = TemplateValue>(
  options: SelectOption<T>[],
  kwargs?: { message?: string },
): T;
type SelectOption<T> = T | { label: string; value: T };

export function sensitive<T>(value: T): T | string;
