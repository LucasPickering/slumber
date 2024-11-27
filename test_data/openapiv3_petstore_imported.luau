local profiles = {
    {
        id = "/v3",
        name = "/v3",
        data = {
            host = "/v3"
        }
    }
}

local requests = {
    {
        id = "pet",
        type = "folder",
        requests = {
            {
                id = "addPet",
                type = "request",
                name = "Add a new pet to the store",
                method = "POST",
                url = "{{profile().host}}/pet",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            },
            {
                id = "updatePet",
                type = "request",
                name = "Update an existing pet",
                method = "PUT",
                url = "{{profile().host}}/pet",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            },
            {
                id = "findPetsByStatus",
                type = "request",
                name = "Finds Pets by status",
                method = "GET",
                url = "{{profile().host}}/pet/findByStatus",
                body = {},
                authentication = {},
                query = {
                    status = ""
                },
                headers = {}
            },
            {
                id = "findPetsByTags",
                type = "request",
                name = "Finds Pets by tags",
                method = "GET",
                url = "{{profile().host}}/pet/findByTags",
                body = {},
                authentication = {},
                query = {
                    tags = ""
                },
                headers = {}
            },
            {
                id = "deletePet",
                type = "request",
                name = "Deletes a pet",
                method = "DELETE",
                url = "{{profile().host}}/pet/{{petId}}",
                body = {},
                authentication = {},
                query = {},
                headers = {
                    api_key = ""
                }
            },
            {
                id = "getPetById",
                type = "request",
                name = "Find pet by ID",
                method = "GET",
                url = "{{profile().host}}/pet/{{petId}}",
                body = {},
                authentication = {},
                query = {},
                headers = {
                    api_key = "{{api_key}}"
                }
            },
            {
                id = "updatePetWithForm",
                type = "request",
                name = "Updates a pet in the store with form data",
                method = "POST",
                url = "{{profile().host}}/pet/{{petId}}",
                body = {},
                authentication = {},
                query = {
                    name = "",
                    status = ""
                },
                headers = {}
            },
            {
                id = "uploadFile",
                type = "request",
                name = "uploads an image",
                method = "POST",
                url = "{{profile().host}}/pet/{{petId}}/uploadImage",
                body = {},
                authentication = {},
                query = {
                    additionalMetadata = ""
                },
                headers = {}
            }
        }
    },
    {
        id = "store",
        type = "folder",
        requests = {
            {
                id = "getInventory",
                type = "request",
                name = "Returns pet inventories by status",
                method = "GET",
                url = "{{profile().host}}/store/inventory",
                body = {},
                authentication = {},
                query = {},
                headers = {
                    api_key = "{{api_key}}"
                }
            },
            {
                id = "placeOrder",
                type = "request",
                name = "Place an order for a pet",
                method = "POST",
                url = "{{profile().host}}/store/order",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            },
            {
                id = "deleteOrder",
                type = "request",
                name = "Delete purchase order by ID",
                method = "DELETE",
                url = "{{profile().host}}/store/order/{{orderId}}",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            },
            {
                id = "getOrderById",
                type = "request",
                name = "Find purchase order by ID",
                method = "GET",
                url = "{{profile().host}}/store/order/{{orderId}}",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            }
        }
    },
    {
        id = "user",
        type = "folder",
        requests = {
            {
                id = "createUser",
                type = "request",
                name = "Create user",
                method = "POST",
                url = "{{profile().host}}/user",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            },
            {
                id = "createUsersWithListInput",
                type = "request",
                name = "Creates list of users with given input array",
                method = "POST",
                url = "{{profile().host}}/user/createWithList",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            },
            {
                id = "loginUser",
                type = "request",
                name = "Logs user into the system",
                method = "GET",
                url = "{{profile().host}}/user/login",
                body = {},
                authentication = {},
                query = {
                    username = "",
                    password = ""
                },
                headers = {}
            },
            {
                id = "logoutUser",
                type = "request",
                name = "Logs out current logged in user session",
                method = "GET",
                url = "{{profile().host}}/user/logout",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            },
            {
                id = "deleteUser",
                type = "request",
                name = "Delete user",
                method = "DELETE",
                url = "{{profile().host}}/user/{{username}}",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            },
            {
                id = "getUserByName",
                type = "request",
                name = "Get user by user name",
                method = "GET",
                url = "{{profile().host}}/user/{{username}}",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            },
            {
                id = "updateUser",
                type = "request",
                name = "Update user",
                method = "PUT",
                url = "{{profile().host}}/user/{{username}}",
                body = {},
                authentication = {},
                query = {},
                headers = {}
            }
        }
    }
}

collection = {
    profiles = profiles,
    requests = requests
}
