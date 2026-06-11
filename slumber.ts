import {
  command,
  file,
  Folder,
  Profiles,
  prompt,
  Recipe,
  Recipes,
  response,
  select,
  sensitive,
  TemplateValue,
} from "./slumber.d.ts";

export const name = "Example";
export const profiles: Profiles = {
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
  initFails: {
    name: "Request Init Fails",
    data: {},
  },
  requestFails: {
    name: "Request Fails",
    data: {
      host: "http://localhost:5000",
    },
  },
};

interface Fish {
  id: number;
  name: string;
}

function getUsername(): string {
  const username = sensitive(command(["whoami"]).trim());
  return `xX${username}Xx`;
}

function getFishId(): number {
  const options = response<Fish[]>("list_fish").map((fish) => ({
    label: fish.name,
    value: fish.id,
  }));
  return select(options, { message: "FishID" });
}

const baseRecipe: Partial<Recipe> = {
  headers: {
    Accept: "application/json",
  },
};
const authenticated: Partial<Recipe> = {
  ...baseRecipe,
  authentication: {
    type: "bearer",
    token: () => response<{ id: string }>("new_session", { trigger: "1h" }).id,
  },
};
const weights = [1.2, 3.4, 5.6];

// TODO move this to common code somewhere
function folder(name: string, recipes: Recipes): Folder {
  return { name, recipes };
}

export const recipes: Recipes = {
  test: command(["TODO"]),
  newSession: {
    ...baseRecipe,
    method: "POST",
    url: ({ host }) => `${host}/login`,
  },

  fish: folder("Fish", {
    listFish: {
      ...authenticated,
      name: "List Fish",
      method: "GET",
      url: ({ host }) => `${host}/fish`,
      query: {
        foo: "bar",
      },
    },
    getFish: {
      ...authenticated,
      name: "Get Fish",
      method: "GET",
      url: ({ host }) => `${host}/fish/${getFishId()}`,
      query: {
        select: () =>
          select([
            "foo",
            "bar",
            "baz",
            "a really really really really long option",
            getUsername(),
          ]),
      },
    },
    createFish: {
      ...authenticated,
      name: "Create Fish",
      method: "POST",
      url: ({ host }) => `${host}/fish`,
      body: {
        type: "json",
        data: {
          name: () => prompt({ message: "Name", default: "Fishy" }),
          species: () =>
            select(["Sunfish", "baby fuckin wheel"], { message: "Species" }),
          // TODO convert to int
          age: () => prompt({ message: "Age", default: "3" }),
          weight_kg: () => select(weights, { message: "Weight (kg)" }),
        },
      },
    },
    modifyFish: {
      ...authenticated,
      name: "Modify Fish",
      method: "PATCH",
      url: ({ host }) => `${host}/fish/${getFishId()}`,
      body: {
        type: "json",
        data: {
          // TODO dedupe with createFish
          age: () => prompt({ message: "Age", default: "3" }),
          weight_kg: () =>
            // Prompt should be deferrable, but that's not an option (yet)
            select([...weights as TemplateValue[], {
              label: "Prompt",
              value: prompt({ message: "Weight (kg)" }),
            }], { message: "Weight (kg)" }),
        },
      },
    },
    deleteFish: {
      name: "Delete Fish",
      method: "DELETE",
      url: ({ host }) => `${host}/fish/${getFishId()}`,
    },
  }),

  authentication: folder(
    "Authentication",
    (() => {
      const common: Recipe = {
        ...baseRecipe,
        persist: false,
        method: "POST",
        url: ({ host }) => `${host}/anything/login`,
        query: {
          sudo: "yes_please",
          fast: ["no_thanks", "actually_maybe"],
        },
      };
      return {
        basicAuth: {
          ...common,
          authentication: {
            type: "basic",
            username: () => getUsername(),
            password: () => prompt({ message: "Password" }),
          },
        },
        bearerAuth: {
          ...common,
          authentication: {
            type: "bearer",
            token: "auth_token",
          },
        },
      };
    })(),
  ),

  bodies: folder("Misc Bodies", {
    streamFile: {
      name: "Stream File",
      method: "POST",
      url: ({ host }) => `${host}/anything`,
      body: () => file("Cargo.toml", { output: "stream" }),
    },
    streamCommand: {
      name: "Stream Command",
      method: "POST",
      url: ({ host }) => `${host}/anything`,
      body: () => command(["cat", "Cargo.toml"]),
    },
    // Compound streaming (via string templates) doesn't work (JS semantics)
    formUrlencoded: {
      name: "Form URL-encoded",
      method: "POST",
      url: ({ host }) => `${host}/anything`,
      body: {
        type: "formUrlencoded",
        data: {
          username: getUsername,
          password: () =>
            prompt({
              "message": "Password",
              default: "hunter2",
              sensitive: true,
            }),
        },
      },
    },
    formMultipart: {
      name: "Form Multipart",
      method: "POST",
      url: ({ host }) => `${host}/anything`,
      body: {
        type: "formMultipart",
        data: {
          image: () => {
            const path = prompt({
              message: "Path",
              default: "static/slumber.png",
            });
            return file(path, { output: "bytes" });
          },
        },
      },
    },
    rawJson: {
      name: "Raw JSON",
      method: "POST",
      url: ({ host }) => `${host}/anything`,
      headers: {
        "Content-Type": "application/json",
      },
      body: `{
        "location": "boston",
        "size": "BOSTON"
      }`,
    },
    largeJson: {
      name: "Large JSON",
      method: "POST",
      url: ({ host }) => `${host}/anything`,
      body: {
        type: "json",
        data: {
          "null": null,
          int: 3,
          float: 4.32,
          bool: false,
          string: "hello",
          expression: getUsername,
          template: () => `${getUsername()}`,
          object: { a: 1, nested: { b: 2, nested: { c: [3, 4, 5] } } },
        },
      },
    },
    largeBody: {
      name: "Large Body",
      method: "POST",
      url: ({ host }) => `${host}/anything`,
      body: () => {
        const numBytes = prompt({ "message": "Bytes", default: "500000" });
        return command([
          "sh",
          "-c",
          `head -c ${numBytes} < /dev/urandom | base64`,
        ], { output: "stream" });
      },
    },
  }),
};
