// TODO doc comments on everything

export type Primitive = null | boolean | string | number;
export type Complex<T> =
  | T
  | Array<Complex<T>>
  | { [key: string]: Complex<T> }
  | Promise<Complex<T>>;
export type Value = Complex<Primitive>;
// TODO is Dynamic the best name? maybe rename again
export type Dynamic<P extends ProfileData, T = Primitive> =
  | T
  // TODO explain why we don't return Dynamic<T>
  | ((profile: P) => Complex<T>)
  // TODO explain
  | Array<Dynamic<P, T>>
  | { [key: string]: Dynamic<P, T> };
export type DynamicBinary<P extends ProfileData> = Dynamic<
  P,
  Primitive | Bytes
>;
export type DynamicStream<P extends ProfileData> = Dynamic<
  P,
  Primitive | Bytes | Stream
>;

export type Profiles<D extends ProfileData = ProfileData> = {
  [id: string]: Profile<D>;
};
export type ProfileData = { [field: string]: Value };

export interface Profile<D extends ProfileData = ProfileData> {
  name?: string;
  default?: boolean;
  // Only static values are supported here; no templates
  data?: D;
}

export type Recipes<P extends ProfileData = ProfileData> = {
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
  url: Dynamic<P>;
  query?: { [header: string]: Dynamic<P> | Dynamic<P>[] };
  headers?: { [header: string]: DynamicBinary<P> };
  body?:
    | DynamicStream<P>
    | { type: "formUrlencoded"; data: { [field: string]: Dynamic<P> } }
    | { type: "formMultipart"; data: { [field: string]: DynamicStream<P> } }
    | { type: "json"; data: Dynamic<P> };
  authentication?:
    | { type: "basic"; username: Dynamic<P>; password?: Dynamic<P> }
    | { type: "bearer"; token: Dynamic<P> };
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

export type Bytes = { $type: "bytes" };
export type Stream = { $type: "stream" };

export function command(
  command: string[],
  kwargs?: CommandKwargs & { output?: "string" },
): Promise<string>;
export function command(
  command: string[],
  kwargs: CommandKwargs & { output: "bytes" },
): Promise<Bytes>;
export function command(
  command: string[],
  kwargs: CommandKwargs & { output: "stream" },
): Promise<Stream>;

interface CommandKwargs {
  cwd?: string;
  output?: "string" | "bytes" | "stream";
  stdin?: string | Bytes;
}

export function file(
  path: string,
  kwargs?: { output?: "string" },
): Promise<string>;
export function file(path: string, kwargs: { output: "bytes" }): Promise<Bytes>;
export function file(
  path: string,
  kwargs: { output: "stream" },
): Promise<Stream>;

export function prompt(
  kwargs?: { message?: string; default?: string; sensitive?: boolean },
): Promise<string>;

export function response<T>(
  recipeId: string,
  kwargs?: ResponseKwargs & { output?: "todo" },
): Promise<T>;
// When not decoding, the response can bytes or UTF-8 string
export function response(
  recipeId: string,
  kwargs: ResponseKwargs & { output: "string" },
): Promise<string>;
export function response(
  recipeId: string,
  kwargs: ResponseKwargs & { output: "bytes" },
): Promise<Bytes>;
interface ResponseKwargs {
  trigger?: "never" | "noHistory" | "always" | string;
  output?: "todo" | "string" | "bytes";
}

export function responseHeader(
  recipeId: string,
  header: string,
  kwargs?: ResponseKwargs & { output?: "string" },
): Promise<string>;
export function responseHeader(
  recipeId: string,
  header: string,
  kwargs?: ResponseKwargs & { output: "bytes" },
): Promise<Bytes>;

// Generics allows for type restriction when doing operations on the output
export function select<T extends Value = Value>(
  options: SelectOption<T>[],
  kwargs?: { message?: string },
): Promise<T>;
type SelectOption<T> = T | { label: string; value: T };

export function sensitive<T>(value: T): T | string;
