import {
  command,
  file,
  jsonPath,
  profile,
  prompt,
  response,
  select,
} from "slumber";

const chain_username = () => command(["whoami"]).trim();
const chain_password = () => prompt({ message: "Password", sensitive: true });
const chain_login_form_values = () =>
  jsonPath("$.form[*]", JSON.parse(response("login", { trigger: "12h" })), {
    mode: "array",
  });
const chain_auth_token = () =>
  jsonPath("$.form", JSON.parse(response("login", { trigger: "12h" })));
const chain_image = () => file("./static/slumber.png");
const chain_big_file = () => file("Cargo.lock");

// These get bumped to the bottom of the list because of their dependencies on
// other chains
const chain_select_value = () =>
  select(
    [
      "foo",
      "bar",
      "baz",
      "a really really really really long option",
      chain_username(),
    ],
    { message: "Select a value" }
  );
const chain_select_dynamic = () =>
  select(chain_login_form_values(), { message: "Select a value" });

export const profiles = {
  works: {
    name: "This Works",
    default: true,
    data: {
      host: "https://httpbin.org",
      username: () => `xX${chain_username()}Xx`,
      user_guid: "abc123",
    },
  },
  ["init-fails"]: {
    data: {},
  },
  ["request-fails"]: {
    name: "Request Fails",
    data: {
      host: "http://localhost:5000",
      username: () => `xX${chain_username()}Xx`,
      user_guid: "abc123",
    },
  },
};

export const requests = {
  login: {
    type: "request",
    persist: false,
    method: "POST",
    url: () => `${profile("host")}/anything/login`,
    query: {
      sudo: "yes_please",
      dupe_static: ["no_thanks", "actually_maybe"],
      dupe_dynamic: () => ["static", profile("username")],
    },
    headers: {
      accept: "application/json",
    },
    authentication: {
      type: "basic",
      username: () => profile("username"),
      password: () => chain_password(),
    },
    body: {
      type: "formUrlencoded",
      data: {
        username: () => profile("username"),
        password: () => chain_password(),
      },
    },
  },
  users: {
    type: "folder",
    name: "Users",
    requests: {
      get_users: {
        type: "request",
        name: "Get Users",
        method: "GET",
        url: () => `${profile("host")}/get`,
        query: {
          foo: "bar",
          select_static: () => chain_select_value(),
          select_dynamic: () => chain_select_dynamic(),
        },
        headers: {
          accept: "application/json",
        },
        authentication: {
          type: "bearer",
          token: () => chain_auth_token(),
        },
      },
      get_user: {
        type: "request",
        name: "Get User",
        method: "GET",
        url: () => `${profile("host")}/anything/${profile("user_guid")}`,
        headers: {
          accept: "application/json",
        },
        authentication: {
          type: "bearer",
          token: () => chain_auth_token(),
        },
      },
      static_json: {
        type: "request",
        method: "PUT",
        url: () => `${profile("host")}/anything/${profile("user_guid")}`,
        headers: {
          accept: "application/json",
        },
        authentication: {
          type: "bearer",
          token: () => chain_auth_token(),
        },
        body: {
          type: "json",
          data: {
            new_username: "user formerly known as ken",
            number: 3,
            bool: true,
            null: null,
            array: [1, 2, false, 3.3, "www.www"],
          },
        },
      },
      dynamic_json: {
        type: "request",
        method: "PUT",
        url: () => `${profile("host")}/anything/${profile("user_guid")}`,
        headers: {
          accept: "application/json",
        },
        authentication: {
          type: "bearer",
          token: () => chain_auth_token(),
        },
        body: {
          type: "json",
          data: () => ({
            new_username: `user formerly known as ${chain_username()}`,
            number: 3,
            bool: true,
            null: null,
            array: [1, 2, false, 3.3, "www.www"],
          }),
        },
      },
    },
  },
  get_image: {
    type: "request",
    name: "Get Image",
    method: "GET",
    url: () => `${profile("host")}/image`,
    headers: {
      accept: "image/png",
    },
  },
  upload_image: {
    type: "request",
    name: "Upload Image",
    method: "POST",
    url: () => `${profile("host")}/anything/image`,
    body: {
      type: "formMultipart",
      data: {
        filename: "logo.png",
        image: () => chain_image(),
      },
    },
  },
  big_file: {
    type: "request",
    name: "Big File",
    method: "POST",
    url: () => `${profile("host")}/anything`,
    body: () => chain_big_file(),
  },
  dynamic_text: {
    type: "request",
    name: "Dynamic Text",
    method: "POST",
    url: () => `${profile("host")}/anything`,
    body: () => chain_username(),
  },
  form_multipart: {
    type: "request",
    name: "Form Multipart",
    method: "POST",
    url: () => `${profile("host")}/anything`,
    body: {
      type: "formMultipart",
      data: {
        username: () => profile("username"),
        image: () => chain_image(),
      },
    },
  },
  raw_json: {
    type: "request",
    name: "Raw JSON",
    method: "POST",
    url: () => `${profile("host")}/anything`,
    headers: {
      ["content-type"]: "application/json",
    },
    body: '{"location": "boston", "size": "HUGE"}',
  },
  delay: {
    type: "request",
    name: "Delay",
    method: "GET",
    url: () => `${profile("host")}/delay/5`,
    headers: {
      accept: "application/json",
    },
    authentication: {
      type: "bearer",
      token: () => chain_auth_token(),
    },
  },
};
