declare module "slumber" {
  // TODO support bytes
  function command(command: Value[]): string;
  function float(value: Value): number;
  function file(path: Value): string;
  function integer(value: Value): number;
  function prompt(
    kwargs?: { message?: string; default?: Value; sensitive?: boolean },
  ): string;
  function response<T = unknown>(
    recipe_id: string,
    kwargs?: { trigger?: "always" | "no_history" | "never" | Duration },
  ): T;
  function select<T extends Value = Value>(
    options: Array<T | { label: Value; value: T }>,
    kwargs?: { message?: string; default?: T },
  ): T;
  function sensitive<T>(value: T): T;

  // Body fns
  function json(data: Value): { type: "json"; data: Value };
  function stream(data: Value): { type: "stream"; data: Value };

  type Value<PD extends ProfileData = ProfileData> =
    | null
    | number
    | boolean
    | string
    | Value[]
    | { [key: string]: Value }
    | ((profile: PD) => Value);
  // TODO make this stricter
  // https://www.typescriptlang.org/docs/handbook/2/template-literal-types.html#intrinsic-string-manipulation-types
  type Duration = string;

  interface Collection {
    name?: string;
    profiles: Profiles;
    requests: Recipes;
  }

  type Profiles = { [id: string]: Profile };

  interface Profile {
    name?: string;
    default?: boolean;
    data: ProfileData;
  }

  type ProfileData = {
    [field: string]: Value;
  };

  type Recipes<PD extends ProfileData = ProfileData> = {
    [id: string]: Recipe<PD> | Folder<PD>;
  };

  interface Folder<PD extends ProfileData = ProfileData> {
    name?: string;
    requests: Recipes<PD>;
  }

  interface Recipe<PD extends ProfileData = ProfileData> {
    name?: string;
    persist?: boolean;
    url: Value;
    method:
      | "CONNECT"
      | "DELETE"
      | "GET"
      | "HEAD"
      | "OPTIONS"
      | "PATCH"
      | "POST"
      | "PUT"
      | "TRACE";
    query?: { [param: string]: Value<PD> | Value<PD>[] };
    authentication?:
      | { type: "basic"; username: Value<PD>; password?: Value<PD> }
      | { type: "bearer"; token: Value<PD> };
    headers?: { [header: string]: Value<PD> };
    body?:
      | Value<PD>
      | { type: "json"; data: Value<PD> }
      | { type: "form_urlencoded"; data: { [field: string]: Value<PD> } }
      | { type: "form_multipart"; data: { [field: string]: Value<PD> } }
      | { type: "stream"; data: Value<PD> };
  }
}
