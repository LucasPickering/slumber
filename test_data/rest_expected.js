import { file, profile } from "slumber";

export const profiles = {
  http_file: {
    name: "Jetbrains HTTP File",
    default: true,
    data: {
      HOST: "http://httpbin.org",
      FIRST: "Joe",
      LAST: "Smith",
      FULL: () => `${profile("FIRST")} ${profile("LAST")}`,
      ENDPOINT: "post",
    },
  },
};

export const requests = {
  SimpleGet_0: {
    type: "request",
    name: "SimpleGet",
    method: "GET",
    url: () => `${profile("HOST")}/get`,
  },
  JsonPost_1: {
    type: "request",
    name: "JsonPost",
    method: "POST",
    url: () => `${profile("HOST")}/post`,
    query: {
      hello: "123",
    },
    headers: {
      ["content-type"]: "application/json",
      ["X-Http-Method-Override"]: "PUT",
    },
    authentication: {
      type: "basic",
      username: "foo",
      password: "bar",
    },
    body: {
      type: "json",
      data: () => ({
        data: "my data",
        name: profile("FULL"),
      }),
    },
  },
  request_2: {
    type: "request",
    method: "POST",
    url: () => `https://httpbin.org/${profile("ENDPOINT")}`,
    headers: {
      ["content-type"]: "application/x-www-form-urlencoded",
      ["my-header"]: "hello",
      ["other-header"]: "goodbye",
    },
    authentication: {
      type: "bearer",
      token: "efaxijasdfjasdfa",
    },
    body: {
      type: "formUrlencoded",
      data: {
        first: () => profile("FIRST"),
        last: () => profile("LAST"),
        full: () => profile("FULL"),
      },
    },
  },
  "Pet.json_3": {
    type: "request",
    name: "Pet.json",
    method: "POST",
    url: () => `${profile("HOST")}/post`,
    headers: {
      ["content-type"]: "application/json",
    },
    body: () => file("./test_data/rest_pets.json"),
  },
};
