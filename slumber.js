const profiles = {
  works: {
    name: "This Works",
    default: true,
    data: {
      host: "https://httpbin.org",
      username: "xX{{chains.username}}Xx",
      user_guid: "abc123",
    },
  },
  "init-fails": {
    name: "Request Init Fails",
    data: {},
  },
  "request-fails": {
    name: "Request Fails",
    data: {
      host: "http://localhost:5000",
      username: "xX{{chains.username}}Xx",
      user_guid: "abc123",
    },
  },
};

const chains = {
  username: {
    source: { type: "command", command: ["whoami"] },
    trim: "both",
  },
  password: {
    source: { type: "prompt", message: "Password" },
    sensitive: true,
  },
  select_value: {
    source: {
      type: "select",
      message: "Select a value",
      options: [
        "foo",
        "bar",
        "baz",
        "a really really really really long option",
        "{{chains.username}}",
      ],
    },
  },
  select_dynamic: {
    source: {
      type: "select",
      message: "Select a value",
      options: "{{chains.login_form_values}}",
    },
  },
  login_form_values: {
    source: { type: "request", recipe: "login" },
    selector: "$.form[*]",
    selector_mode: "array",
  },
  auth_token: {
    source: { type: "request", recipe: "login", trigger: { expire: "12h" } },
    selector: "$.form",
  },
  image: {
    source: { type: "file", path: "./static/slumber.png" },
  },
  big_file: {
    source: { type: "file", path: "Cargo.lock" },
  },
};

const recipeBase = {
  authentication: { bearer: "{{chains.auth_token}}" },
  headers: {
    Accept: "application/json",
    "Content-Type": "application/json",
  },
};

const requests = {
  login: {
    request: {
      method: "POST",
      url: "{{host}}/anything/login",
      authentication: {
        basic: { username: "{{username}}", password: "{{chains.password}}" },
      },
      // query: ["sudo=yes_please", "fast=no_thanks", "fast=actually_maybe"],
      headers: { Accept: "application/json" },
      body: {
        type: "form_urlencoded",
        username: "{{username}}",
        password: "{{chains.password}}",
      },
    },
  },
  users: {
    folder: {
      name: "Users",
      requests: {
        get_users: {
          request: {
            ...recipeBase,
            name: "Get Users",
            method: "GET",
            url: "{{host}}/get",
            // query: ["foo=bar", "select={{chains.select_dynamic}}"],
          },
        },
        get_user: {
          request: {
            ...recipeBase,
            name: "Get User",
            method: "GET",
            url: "{{host}}/anything/{{user_guid}}",
          },
        },
        modify_user: {
          request: {
            ...recipeBase,
            name: "Modify User",
            method: "PUT",
            url: "{{host}}/anything/{{user_guid}}",
            // TODO JSON body
            // body: {
            //     new_username: "user formerly known as {{chains.username}}",
            //     number: 3,
            //     bool: true,
            //     null: null,
            //     array: [1, 2, false, 3.3, "www.www"],
            // },
          },
        },
      },
    },
  },
  get_image: {
    request: {
      headers: { Accept: "image/png" },
      name: "Get Image",
      method: "GET",
      url: "{{host}}/image",
    },
  },
  upload_image: {
    request: {
      name: "Upload Image",
      method: "POST",
      url: "{{host}}/anything/image",
      body: {
        type: "form_multipart",
        filename: "logo.png",
        image: "{{chains.image}}",
      },
    },
  },
  big_file: {
    request: {
      name: "Big File",
      method: "POST",
      url: "{{host}}/anything",
      // TODO accept a plain value
      body: { type: "raw", body: "{{chains.big_file}}" },
    },
  },
  delay: {
    request: {
      ...recipeBase,
      name: "Delay",
      method: "GET",
      url: "{{host}}/delay/5",
    },
  },
};

export default () => ({ profiles, chains, requests });
