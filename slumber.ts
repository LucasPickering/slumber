import {
  command,
  Profiles,
  prompt,
  Recipe,
  Recipes,
  response,
  select,
  sensitive,
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

export const recipes: Recipes = {
  newSession: {
    ...baseRecipe,
    method: "POST",
    url: ({ host }) => `${host}/login`,
  },
  // TODO merge common fields into a fn scope?
  fish: {
    recipes: {
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
              select([...weights, {
                label: "Prompt",
                value: parseInt(prompt({ message: "Weight (kg)" })),
              }], { message: "Weight (kg)" }),
          },
        },
      },
      deleteFish: {
        name: "Delete Fish",
        method: "DELETE",
        url: ({ host }) => `${host}/fish/${getFishId()}`,
      },
    },
  },

  authentication: {
    name: "Authentication",
    recipes: (() => {
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
  },
};
