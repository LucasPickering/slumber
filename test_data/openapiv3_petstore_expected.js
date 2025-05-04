import { profile } from "slumber";

export const profiles = {
  ["/v3"]: {
    name: "/v3",
    data: {
      host: "/v3",
    },
  },
};

export const requests = {
  ["tag/pet"]: {
    type: "folder",
    name: "pet",
    requests: {
      addPet: {
        type: "request",
        name: "Add a new pet to the store",
        method: "POST",
        url: () => `${profile("host")}/pet`,
      },
      updatePet: {
        type: "request",
        name: "Update an existing pet",
        method: "PUT",
        url: () => `${profile("host")}/pet`,
      },
      findPetsByStatus: {
        type: "request",
        name: "Finds Pets by status",
        method: "GET",
        url: () => `${profile("host")}/pet/findByStatus`,
        query: {
          status: "",
        },
      },
      findPetsByTags: {
        type: "request",
        name: "Finds Pets by tags",
        method: "GET",
        url: () => `${profile("host")}/pet/findByTags`,
        query: {
          tags: "",
        },
      },
      deletePet: {
        type: "request",
        name: "Deletes a pet",
        method: "DELETE",
        url: () => `${profile("host")}/pet/${profile("petId")}`,
        headers: {
          api_key: "",
        },
      },
      getPetById: {
        type: "request",
        name: "Find pet by ID",
        method: "GET",
        url: () => `${profile("host")}/pet/${profile("petId")}`,
        headers: {
          api_key: () => profile("api_key"),
        },
      },
      updatePetWithForm: {
        type: "request",
        name: "Updates a pet in the store with form data",
        method: "POST",
        url: () => `${profile("host")}/pet/${profile("petId")}`,
        query: {
          name: "",
          status: "",
        },
      },
      uploadFile: {
        type: "request",
        name: "uploads an image",
        method: "POST",
        url: () => `${profile("host")}/pet/${profile("petId")}/uploadImage`,
        query: {
          additionalMetadata: "",
        },
      },
    },
  },
  ["tag/store"]: {
    type: "folder",
    name: "store",
    requests: {
      getInventory: {
        type: "request",
        name: "Returns pet inventories by status",
        method: "GET",
        url: () => `${profile("host")}/store/inventory`,
        headers: {
          api_key: () => profile("api_key"),
        },
      },
      placeOrder: {
        type: "request",
        name: "Place an order for a pet",
        method: "POST",
        url: () => `${profile("host")}/store/order`,
      },
      deleteOrder: {
        type: "request",
        name: "Delete purchase order by ID",
        method: "DELETE",
        url: () => `${profile("host")}/store/order/${profile("orderId")}`,
      },
      getOrderById: {
        type: "request",
        name: "Find purchase order by ID",
        method: "GET",
        url: () => `${profile("host")}/store/order/${profile("orderId")}`,
      },
    },
  },
  ["tag/user"]: {
    type: "folder",
    name: "user",
    requests: {
      createUser: {
        type: "request",
        name: "Create user",
        method: "POST",
        url: () => `${profile("host")}/user`,
      },
      createUsersWithListInput: {
        type: "request",
        name: "Creates list of users with given input array",
        method: "POST",
        url: () => `${profile("host")}/user/createWithList`,
      },
      loginUser: {
        type: "request",
        name: "Logs user into the system",
        method: "GET",
        url: () => `${profile("host")}/user/login`,
        query: {
          username: "",
          password: "",
        },
      },
      logoutUser: {
        type: "request",
        name: "Logs out current logged in user session",
        method: "GET",
        url: () => `${profile("host")}/user/logout`,
      },
      deleteUser: {
        type: "request",
        name: "Delete user",
        method: "DELETE",
        url: () => `${profile("host")}/user/${profile("username")}`,
      },
      getUserByName: {
        type: "request",
        name: "Get user by user name",
        method: "GET",
        url: () => `${profile("host")}/user/${profile("username")}`,
      },
      updateUser: {
        type: "request",
        name: "Update user",
        method: "PUT",
        url: () => `${profile("host")}/user/${profile("username")}`,
      },
    },
  },
};
