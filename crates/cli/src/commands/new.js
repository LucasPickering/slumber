// For basic usage info, see:
// https://slumber.lucaspickering.me/book/getting_started.html
// For all collection options, see:
// https://slumber.lucaspickering.me/book/api/request_collection/index.html

import { profile, response } from "slumber";

// Profiles are groups of data you can easily switch between. A common usage is
// to define profiles for various environments of a REST service
export const profiles = {
  example: {
    name: "Example Profile",
    data: {
      host: "https://httpbin.org",
      // TODO show off dynamic profile values
    },
  },
};

export const requests = {
  example1: {
    type: "request",
    name: "Example Request 1",
    persist: true,
    method: "GET",
    // Functions can be used to define dynamic data. Here the URL is built
    // using the `host` field from the selected profile
    url: () => `${profile("host")}/anything`,
  },
  example_folder: {
    type: "folder",
    name: "Example Folder",
    requests: {
      example2: {
        type: "request",
        name: "Example Request 2",
        persist: true,
        method: "POST",
        url: () => `${profile("host")}/anything`,
        body: {
          type: "json",
          // Here's an example of pulling data from one response and using it
          // in a subsequent request
          data: () => JSON.parse(response("example1")).data,
        },
      },
    },
  },
};
