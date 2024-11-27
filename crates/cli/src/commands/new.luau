-- For basic usage info, see:
-- https://slumber.lucaspickering.me/book/getting_started.html
-- For all collection options, see:
-- https://slumber.lucaspickering.me/book/api/request_collection/index.html

-- Profiles are groups of data you can easily switch between. A common usage is
-- to define profiles for various environments of a REST service
local profiles = {
    {
        id = "example",
        name = "Example Profile",
        data = {
            host = "https://httpbin.org",
        },
    }
}

-- This custom function accesses the response body of the "example1" recipe, and
-- extracts the "data" field
function response_body()
    local body = response({recipe = "example1"})
    return json_path({query = "$.data", data = body})
end

local requests = {
    {
        type = "request",
        id = "example1",
        name = "Example Request 1",
        method = "GET",
        -- Templates can be used for dynamic data. Anything inside {{ }} is a
        -- Lua expression. The profile() function returns the `data` field of
        -- the selected profile.
        url = "{{profile().host}}/anything",
    },
    {
        type = "folder",
        name = "Example Folder",
        requests = {
            {
                id = "example2",
                name = "Example Request 2",
                method = "POST",
                url = "{{profile().host}}/anything",
                body = {
                    type = "json",
                    -- Templates can contain any Lua expression, including
                    -- custom functions
                    data = "{{response_body()}}"
                }
            },
        }
    }
}

collection = {profiles = profiles, requests = requests}
