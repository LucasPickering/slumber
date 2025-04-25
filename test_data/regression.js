import { profile, prompt } from "slumber";

const username = () => prompt({ message: "Username" });
const password = () => prompt({ message: "Password", sensitive: true });

export const profiles = {
  profile1: {
    name: "Profile 1",
    default: false,
    data: {
      userGuid: "abc123",
      username: () => `xX${username()}Xx`,
      host: "https://httpbin.org",
    },
  },
  profile2: {
    name: "Profile 2",
    default: true,
    data: {
      host: "https://httpbin.org",
    },
  },
};

export const requests = {
  textBody: {
    type: "request",
    method: "POST",
    url: () => `${profile("host")}/anything/login`,
    query: {
      sudo: "yes_please",
      fast: "no_thanks",
    },
    headers: {
      accept: "application/json",
    },
    body: () =>
      `{"username": "${profile("username")}", "password": "${password()}"}`,
  },
  users: {
    type: "folder",
    name: "Users",
    requests: {
      simple: {
        type: "request",
        persist: false,
        name: "Get User",
        method: "GET",
        url: () => `${profile("host")}/anything/${profile("userGuid")}`,
        query: {
          value: [() => profile("field1"), () => profile("field2")],
        },
      },
      jsonBody: {
        type: "request",
        name: "Modify User",
        method: "PUT",
        url: () => `${profile("host")}/anything/${profile("userGuid")}`,
        headers: {
          accept: "application/json",
        },
        authentication: {
          type: "bearer",
          token: () => authToken(),
        },
        body: {
          type: "json",
          data: {
            username: "new username",
          },
        },
      },
      jsonBodyButNot: {
        type: "request",
        name: "Modify User",
        method: "PUT",
        url: () => `${profile("host")}/anything/${profile("userGuid")}`,
        headers: {
          accept: "application/json",
        },
        authentication: {
          type: "basic",
          username: () => profile("username"),
          password,
        },
        body: {
          type: "json",
          data: `{"warning": "NOT an object"}`,
        },
      },
      formUrlencodedBody: {
        type: "request",
        name: "Modify User",
        method: "PUT",
        url: () => `${profile("host")}/anything/${profile("userGuid")}`,
        headers: {
          accept: "application/json",
        },
        body: {
          type: "formUrlencoded",
          data: {
            username: "new username",
            password,
          },
        },
      },
      formMultipartBody: {
        type: "request",
        name: "Modify User",
        method: "PUT",
        url: () => `${profile("host")}/anything/${profile("userGuid")}`,
        headers: {
          accept: "application/json",
        },
        body: {
          type: "formMultipart",
          data: {
            username: "new username",
            password,
          },
        },
      },
    },
  },
};
