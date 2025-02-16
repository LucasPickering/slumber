// TODO make casing consistent

const profiles = {
  works: {
    name: "This Works",
    default: true,
    data: {
      host: "https://httpbin.org",
      username: () => `xX${username()}Xx`,
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

function username() {
  return command({ command: ["whoami"], trim: "both" });
}

function password() {
  return prompt({ message: "Password", sensitive: true });
}

// TODO use this
function selectValue() {
  return select({
    message: "Select a value",
    options: [
      "foo",
      "bar",
      "baz",
      "a really really really really long option",
      username,
    ],
  });
}

function selectDynamic() {
  const options = jsonPath({
    query: "$.form[*]",
    data: response("login"),
  })[0];
  return select({ message: "Select a value", options });
}

function authToken() {
  const response = response("login", {
    trigger: { type: "expire", duration: "12h" },
  });

  // Pick some arbitrary data from the login response as the token
  const token = JSON.stringify(response.form);
  return command({ command: "base64", stdin: token });
}

const recipeBase = {
  authentication: { type: "bearer", token: authToken },
  headers: {
    Accept: "application/json",
    "Content-Type": "application/json",
  },
};

// TODO these should be provided by slumber
function request(recipe) {
  return { request: recipe };
}

function folder(folder) {
  return { folder };
}

const requests = {
  login: request({
    method: "POST",
    url: () => `${profile("host")}/anything/login`,
    authentication: {
      type: "basic",
      username: ({ username }) => username,
      password: password,
    },
    // query: ["sudo=yes_please", "fast=no_thanks", "fast=actually_maybe"],
    headers: { Accept: "application/json" },
    body: {
      // This is duplicated from the authentication header, to demonstrate
      // URL forms
      type: "form_urlencoded",
      username: () => profile("username"),
      password: password,
    },
  }),
  users: folder({
    name: "Users",
    requests: {
      get_users: request({
        ...recipeBase,
        name: "Get Users",
        method: "GET",
        url: () => `${profile("host")}/get`,
        // query: {
        //   foo: "bar",
        //   select: selectDynamic,
        // },
      }),
      get_user: request({
        ...recipeBase,
        name: "Get User",
        method: "GET",
        url: () => `${profile("host")}/anything/${profile("userGuid")}`,
      }),
      modify_user: request({
        ...recipeBase,
        name: "Modify User",
        method: "PUT",
        url: () => `${profile("host")}/anything/${profile("userGuid")}`,
        // body: {
        //   type: "json",
        //   data: () => ({
        //     new_username: `user formerly known as ${username()}`,
        //     number: 3,
        //     bool: true,
        //     null: null,
        //     array: [1, 2, false, 3.3, "www.www"],
        //   }),
        // },
      }),
    },
  }),
  get_image: request({
    headers: { Accept: "image/png" },
    name: "Get Image",
    method: "GET",
    url: () => `${profile("host")}/image`,
  }),
  upload_image: request({
    name: "Upload Image",
    method: "POST",
    url: () => `${profile("host")}/anything/image`,
    body: {
      type: "form_multipart",
      filename: "logo.png",
      image: () => file({ path: "./static/slumber.png" }),
    },
  }),
  big_file: request({
    name: "Big File",
    method: "POST",
    url: () => `${profile("host")}/anything`,
    // TODO accept a plain value
    body: { type: "raw", body: () => file({ path: "Cargo.lock" }) },
  }),
  delay: request({
    ...recipeBase,
    name: "Delay",
    method: "GET",
    url: () => `${profile("host")}/delay/5`,
  }),
};

export default { profiles, requests };
