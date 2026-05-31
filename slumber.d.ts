// TODO doc comments on everything

// TODO is it possible to statically type the available profile fields?
// TODO rename the primtives - they're not templates
type TemplatePrimitive = null | boolean | string | number;
type TemplateValue =
  | TemplatePrimitive
  | TemplateValue[]
  | { [key: string]: TemplateValue };
type Template =
  | TemplatePrimitive
  | ((profile: { [field: string]: unknown }) => Template)
  // TODO explain
  | Template[]
  | { [key: string]: Template };

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
  headers?: { [header: string]: Template };
  body?:
    | { type: "formUrlencoded"; data: { [field: string]: Template } }
    | { type: "formMultipart"; data: { [field: string]: Template } }
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

// TODO bytes
export function command(command: string[]): string;
export function file(path: string): string;
export function prompt(kwargs?: { message?: string; default?: string }): string;
export function response<T>(
  recipeId: string,
  kwargs?: { trigger?: Trigger; decode?: true },
): T;
// When not decoding, the response is always bytes
export function response(
  recipeId: string,
  kwargs: { trigger?: Trigger; decode: false },
): Blob;
export function select<T>(
  options: SelectOption<T>[],
  kwargs?: { message?: string },
): T;
export function sensitive<T>(value: T): T | string;

// TODO better trigger type
type Trigger = string;
type SelectOption<T> = T | { label: string; value: T };
