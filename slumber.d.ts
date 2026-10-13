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
type Template<P extends ProfileData, T = TemplatePrimitive> =
  | T
  // TODO explain why we don't return Template<T>
  | ((profile: P) => Complex<T>)
  // TODO explain
  | Array<Template<P, T>>
  | { [key: string]: Template<P, T> };
type BinaryTemplate<P extends ProfileData> = Template<
  P,
  TemplatePrimitive | Bytes
>;
type StreamTemplate<P extends ProfileData> = Template<
  P,
  TemplatePrimitive | Bytes | Stream
>;

type Profiles<D extends ProfileData = ProfileData> = {
  [id: string]: Profile<D>;
};
type ProfileData = { [field: string]: TemplateValue };

interface Profile<D extends ProfileData = ProfileData> {
  name?: string;
  default?: boolean;
  // Only static values are supported here; no templates
  data?: D;
}

type Recipes<P extends ProfileData = ProfileData> = {
  [id: string]: Folder<P> | Recipe<P>;
};

export interface Folder<P extends ProfileData = ProfileData> {
  name?: string;
  recipes: Recipes<P>;
}

export interface Recipe<P extends ProfileData = ProfileData> {
  name?: string;
  persist?: boolean;
  method: HttpMethod;
  url: Template<P>;
  query?: { [header: string]: Template<P> | Template<P>[] };
  headers?: { [header: string]: BinaryTemplate<P> };
  body?:
    | StreamTemplate<P>
    | { type: "formUrlencoded"; data: { [field: string]: Template<P> } }
    | { type: "formMultipart"; data: { [field: string]: StreamTemplate<P> } }
    | { type: "json"; data: Template<P> };
  authentication?:
    | { type: "basic"; username: Template<P>; password?: Template<P> }
    | { type: "bearer"; token: Template<P> };
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

// Some functions have a `decode` kwarg that controls what type of output is
// returned

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
