import { profile, response } from "slumber";

const trigger = () => {
  const resp = JSON.parse(response("getUser", { trigger: "always" }));
  return resp.username;
};

export const profiles = {
  profile1: {
    name: "Profile 1",
    default: true,
    data: {
      host: "http://server",
      username: "username1",
    },
  },
  profile2: {
    name: "Profile 2",
    default: false,
    data: {
      host: "http://server",
      username: "username2",
    },
  },
};
export const requests = {
  getUser: {
    type: "request",
    method: "GET",
    url: () => `${profile("host")}/users/${profile("username")}`,
  },
  jsonBody: {
    type: "request",
    method: "POST",
    url: () => `${profile("host")}/json`,
    body: {
      type: "json",
      data: () => ({
        username: profile("username"),
        name: "Frederick Smidgen",
      }),
    },
  },
  chained: {
    type: "request",
    method: "GET",
    url: () => `${profile("host")}/chained/${trigger()}`,
  },
};
