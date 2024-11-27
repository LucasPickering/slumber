function username()
    return "whoami"
end

function password()
    return prompt({message = "Password", sensitive = true})
end

local profiles = {
    profile1 = {
        name = "Profile 1",
        data = {
            host = "https://httpbin.org",
            username = "xX{{username()}}Xx",
            user_guid = "abc123"
        }
    },
    profile2 = {
        name = "Profile 2",
        default = true,
        data = {
            host = "https://httpbin.org"
        }
    }
}

local requests = {
    text_body = {
        id = "text_body",
        method = "POST",
        -- Missing name
        url = "{{profile().host}}/anything/login",
        query = {
            sudo = "yes_please",
            fast = "no_thanks"
        },
        headers = {Accept = "application/json"},
        -- Text body
        body = '{"username": "{{username()}}", "password": "{{password()}}"}'
    },

    users = {
        id = "users",
        type = "folder",
        name = "Users",
        requests = {
            simple = {
                id = "simple",
                method = "GET",
                url = "{{profile().host}}/anything/{{profile().user_guid}}",
                -- No headers or authentication
                query = {
                    "value={{profile().field1}}",
                    "value={{profile().field2}}"
                },
                headers = {}
            },

            json_body = {
                id = "json_body",
                method = "PUT",
                url = "{{profile().host}}/anything/{{profile().user_guid}}",
                authentication = {type = "bearer", token = "some_token_value"},
                query = {},
                headers = {Accept = "application/json"},
                body = {type = "json", data = { username = "new username" }},
            },

            json_body_but_not = {
                id = "json_body_but_not",
                method = "PUT",
                url = "{{profile().host}}/anything/{{profile().user_guid}}",
                authentication = {
                    type = "basic",
                    username = "{{username()}}",
                    password = "{{password()}}",
                },
                headers = {Accept = "application/json"},
                -- Nested JSON is *not* parsed
                body = {type = "json", data = '{"warning": "NOT an object"}'},
            },

            form_urlencoded_body = {
                id = "form_urlencoded_body",
                method = "PUT",
                url = "{{profile().host}}/anything/{{profile().user_guid}}",
                headers = {Accept = "application/json"},
                body = {
                    type = "form_urlencoded",
                    data = { username = "new username" },
                }
            }
        }
    }
}

collection = {profiles = profiles, requests = requests}
