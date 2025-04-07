import { command, profile, prompt, response, select } from "slumber";

function username() {
  return command(["whoami"], { trim: "both" });
}

function password() {
  return prompt("Password", { sensitive: true });
}

function selectValue() {
  return select("Select a value", [
    "foo",
    "bar",
    "baz",
    "a really really really really long option",
    username(),
  ]);
}

// Example of generating a selection list dynamically
// TODO use this
function selectDynamic() {
  const resp = response("login", {
    trigger: { type: "expire", duration: "12h" },
  });
  const options = JSON.parse(resp).form;
  return select("Select a value", options);
}

function authToken() {
  const resp = JSON.parse(
    response("login", {
      trigger: { type: "expire", duration: "12h" },
    })
  );

  // Pick some arbitrary data from the login response as the token
  const token = JSON.stringify(resp.form);
  return command(["base64"], { stdin: token, trim: "both" });
}

const recipeBase = {
  authentication: { type: "bearer", token: authToken },
  headers: {
    Accept: "application/json",
    "Content-Type": "application/json",
  },
};

export const profiles = {
  works: {
    name: "This Works",
    default: true,
    data: {
      host: "https://httpbin.org",
      username: () => `xX${username()}Xx`,
      password: password,
      userGuid: "abc123",
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
      username: () => `xX${username()}Xx`,
      userGuid: "abc123",
    },
  },
};

export const requests = {
  login: {
    type: "request",
    method: "POST",
    url: () => `${profile("host")}/anything/login`,
    authentication: {
      type: "basic",
      username: () => profile("username"),
      password: () => profile("password"),
    },
    query: {
      sudo: "yes_please",
      fast: ["no_thanks", "actually_maybe"],
    },
    headers: { Accept: "application/json" },
    body: {
      // This is duplicated from the authentication header, to demonstrate
      // URL forms
      type: "formUrlencoded",
      data: {
        username: () => profile("username"),
        password: () => profile("password"),
      },
    },
  },
  users: {
    type: "folder",
    name: "Users",
    requests: {
      getUsers: {
        type: "request",
        ...recipeBase,
        name: "Get Users",
        method: "GET",
        url: () => `${profile("host")}/get`,
        query: {
          foo: "bar",
          select: selectValue,
        },
      },
      getUser: {
        type: "request",
        ...recipeBase,
        name: "Get User",
        method: "GET",
        url: () => `${profile("host")}/anything/${profile("userGuid")}`,
      },
      modifyUser: {
        type: "request",
        ...recipeBase,
        name: "Modify User",
        method: "PUT",
        url: () => `${profile("host")}/anything/${profile("userGuid")}`,
        body: {
          type: "json",
          data: () => ({
            newUsername: `user formerly known as ${username()}`,
            number: 3,
            bool: true,
            null: null,
            array: [1, 2, false, 3.3, "www.www"],
          }),
        },
      },
    },
  },
  getImage: {
    type: "request",
    headers: { Accept: "image/png" },
    name: "Get Image",
    method: "GET",
    url: () => `${profile("host")}/image`,
  },
  uploadImage: {
    type: "request",
    name: "Upload Image",
    method: "POST",
    url: () => `${profile("host")}/anything/image`,
    body: {
      type: "formMultipart",
      data: {
        filename: "logo.png",
        image: () => file("./static/slumber.png", { decode: "binary" }),
      },
    },
  },
  bigFile: {
    type: "request",
    name: "Big File",
    method: "POST",
    url: () => `${profile("host")}/anything`,
    headers: {
      "Content-Type": "text/plain",
    },
    body: () => file("Cargo.lock"), // Raw text body
  },
  delay: {
    type: "request",
    ...recipeBase,
    name: "Delay",
    method: "GET",
    url: () => `${profile("host")}/delay/5`,
  },
};
