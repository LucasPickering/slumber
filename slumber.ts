import {
  Collection,
  command,
  file,
  json,
  Profiles,
  prompt,
  Recipe,
  Recipes,
  response,
  select,
  sensitive,
  stream,
} from "slumber";

const profiles: Profiles = {
  prd: {
    name: "Production",
    default: true,
    data: {
      host: "https://shoal.lucaspickering.me",
    },
  },
  local: {
    name: "Local",
    data: {
      host: "http://localhost:3000",
    },
  },
  init_fails: {
    name: "Request Init Fails",
    data: {},
  },
  request_fails: {
    name: "Request Fails",
    data: {
      host: "http://localhost:3001",
    },
  },
};

const base_request: Partial<Recipe> = {
  headers: {
    Accept: "application/json",
  },
};
const authenticated: Partial<Recipe> = {
  ...base_request,
  authentication: {
    type: "bearer",
    token: () => response<{ id: string }>("new_session", { trigger: "1h" }).id,
  },
};

function getUsername(): string {
  return `xX${sensitive(command(["whoami"]).trim())}Xx`;
}

function getStreamCompound(): string {
  return `command: ${command(["echo", "test"])}\nfile: ${file("Cargo.toml")}`;
}

function getFishId(): number {
  const options = response<{ id: number; name: string }[]>("list_fish", {
    trigger: "no_history",
  })
    .map(({ name, id }) => ({ label: name, value: id }));
  return select(options, { message: "Fish ID" });
}

const requests: Recipes<Profiles["prd"]["data"]> = {
  new_session: {
    ...base_request,
    name: "New Session",
    method: "POST",
    url: ({ host }) => `${host}/login`,
  },

  fish: {
    name: "Fish",
    requests: {
      list_fish: {
        ...authenticated,
        name: "List Fish",
        method: "GET",
        url: ({ host }) => `${host}/fish`,
        query: {
          foo: "bar",
        },
      },

      get_fish: {
        ...authenticated,
        name: "Get Fish",
        method: "GET",
        url: ({ host }) => `${host}/fish/${getFishId()}`,
        query: {
          select: () =>
            select(
              [
                "foo",
                "bar",
                "a really really really really long option",
                getUsername(),
                () => prompt({ message: "Deferred Prompt" }),
              ],
              { message: "Select a value" },
            ),
        },
      },

      create_fish: {
        ...authenticated,
        name: "Create Fish",
        method: "POST",
        url: ({ host }) => `${host}/fish`,
        body: json({
          name: () => prompt({ message: "Name", default: "Fishy" }),
          species: () =>
            select(["Sunfish", "baby fuckin wheel"], { message: "Species" }),
          age: () => parseInt(prompt({ message: "Age", default: 3 })),
          weight_kg: () => select([1.2, 3.4, 5.6], { message: "Weight (kg)" }),
        }),
      },

      modify_fish: {
        ...authenticated,
        name: "Modify Fish",
        method: "PATCH",
        url: ({ host }) => `${host}/fish/${getFishId()}`,
        body: json({
          age: () => parseInt(prompt({ message: "Age", default: 3 })),
          weight_kg: () =>
            parseFloat(prompt({ message: "Weight (kg)", default: 1.5 })),
        }),
      },

      delete_fish: {
        ...authenticated,
        name: "Delete Fish",
        method: "DELETE",
        url: ({ host, fish_id }) => `${host}/fish/${fish_id}`,
      },
    },
  },

  authentication: {
    name: "Authentication",
    requests: {
      basic_auth: {
        ...base_request,
        method: "POST",
        url: ({ host }) => `${host}/anything/login`,
        persist: false,
        authentication: {
          type: "basic",
          username: () => getUsername(),
          password: () => prompt({ message: "Password", sensitive: true }),
        },
        query: {
          sudo: "yes_please",
          fast: ["no_thanks", "actually_maybe"],
        },
      },
      bearer_auth: {
        ...base_request,
        method: "POST",
        url: ({ host }) => `${host}/anything/login`,
        persist: false,
        authentication: {
          type: "bearer",
          token: "auth_token",
        },
        query: {
          sudo: "yes_please",
          fast: ["no_thanks", "actually_maybe"],
        },
      },
    },
  },

  bodies: {
    name: "Misc Bodies",
    requests: {
      stream_file: {
        ...base_request,
        name: "Stream File",
        method: "POST",
        url: ({ host }) => `${host}/anything`,
        body: stream(() => file("Cargo.toml")),
      },
      stream_command: {
        ...base_request,
        name: "Stream Command",
        method: "POST",
        url: ({ host }) => `${host}/anything`,
        body: stream(() => command(["cat", "Cargo.toml"])),
      },
      stream_compound: {
        ...base_request,
        name: "Stream Compound",
        method: "POST",
        url: ({ host }) => `${host}/anything`,
        body: stream(() => getStreamCompound()),
      },
      form_urlencoded: {
        ...base_request,
        name: "Form URL-encoded",
        method: "POST",
        url: ({ host }) => `${host}/anything`,
        body: {
          type: "form_urlencoded",
          data: {
            username: () => getUsername(),
            password: () =>
              prompt({
                message: "Password",
                default: "hunter2",
                sensitive: true,
              }),
          },
        },
      },
      form_multipart: {
        ...base_request,
        name: "Form Multipart",
        method: "POST",
        url: ({ host }) => `${host}/anything`,
        body: {
          type: "form_multipart",
          data: {
            image: () => {
              const path = prompt({
                message: "Path",
                default: "static/slumber.png",
              });
              return file(path);
            },
          },
        },
      },
      raw_json: {
        ...base_request,
        name: "Raw JSON",
        method: "POST",
        url: ({ host }) => `${host}/anything`,
        body: `{
          "location": "boston",
          "size": "HUGE"
        }`,
      },
      large_json: {
        ...base_request,
        name: "Large JSON",
        method: "POST",
        url: ({ host }) => `${host}/anything`,
        body: json(() => ({
          null: null,
          int: 3,
          float: 4.32,
          bool: false,
          string: "hello",
          // Not actually a stream because JSON is resolved eagerly
          stream: command(["echo", "test"]),
          object: { a: 1, nested: { b: 2, nested: { c: [3, 4, 5] } } },
        })),
      },
      large_body: {
        ...base_request,
        name: "Large Body",
        method: "POST",
        url: ({ host }) => `${host}/anything`,
        body: () => {
          const numBytes = prompt({ message: "Bytes", default: 500000 });
          return command([
            "sh",
            "-c",
            `head -c ${numBytes} < /dev/urandom | base64`,
          ]);
        },
      },
    },
  },

  delay: {
    ...base_request,
    name: "Delay",
    method: "GET",
    url: ({ host }) => `${host}/delay/5`,
  },
  redirect: {
    ...base_request,
    name: "Redirect",
    method: "GET",
    url: ({ host }) => `${host}/redirect-to`,
    query: {
      url: ({ host }) => `${host}/get`,
    },
  },
};

const collection: Collection = {
  name: "Example Collection",
  profiles,
  requests,
};
export default collection;
