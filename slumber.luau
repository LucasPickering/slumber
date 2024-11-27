-- Note: This file is meant primarily for development and testing of Slumber. It
-- is not intended as documentation. While it may be a helpful example, canonical
-- documentation is at: https://slumber.lucaspickering.me/book/

function username()
    return command({command = "whoami", trim = "both"})
end

function password()
    return prompt({message = "Password", sensitive = true})
end

function select_value()
    return choose({
        message = "Select a value",
        options = {
            "foo",
            "bar",
            "baz",
            "a really really really really long option",
            username
        }
    })
end

function select_dynamic()
    local body = response({recipe = "login"})
    -- TODO make this jsonpath work
    local login_form_values = json_path({
        data = body,
        query = "$.headers[*]",
    })
    return choose({
        message = "Select a value",
        options = login_form_values,
    })
end

function auth_token()
    local body = response({
        recipe = "login",
        trigger = {type = "always"}
    })
    return json_path({data = body, query = "$.data"})
end

function big_file()
    return file({path = "./Cargo.lock"})
end

local profiles = {
    {
        id = "works",
        name = "This Works",
        default = true,
        data = {
            host = "https://httpbin.org",
            username = "xX{{username()}}Xx",
            user_guid = "abc123"
        }
    },
    {
        id = "init_fails",
        name = "Request Init Fails",
        data = {}
    },
    {
        id = "request_fails",
        name = "Request Fails",
        data = {
            host = "http://localhost:5000",
            username = "xX{{username()}}Xx",
            user_guid = "abc123"
        }
    }
}

local authentication = {
    type = "bearer",
    token = "{{auth_token()}}"
}
local headers = {
    Accept = "application/json",
    ["Content-Type"] = "application/json"
}
local requests = {
    {
        id = "login",
        type = "request",
        method = "POST",
        url = "{{profile().host}}/anything/login",
        authentication = {
            type = "basic",
            username = "{{profile().username}}",
            password = "{{password()}}",
        },
        query = {
            "sudo=yes_please",
            "fast=no_thanks",
            "fast=actually_maybe",
        },
        body = {
            type = "form_urlencoded",
            data = {
                username = "{{profile().username}}",
                password = "{{password()}}",
            }
        }
    },

    {
        id = "users",
        type = "folder",
        name = "Users",
        requests = {
            {
                id = "get_users",
                type = "request",
                name = "Get Users",
                method = "GET",
                url = "{{profile().host}}/get",
                authentication = authentication,
                headers = headers,
                query = { "foo=bar", "select={{select_dynamic()}}" }
            },

            {
                id = "get_user",
                type = "request",
                name = "Get User",
                method = "GET",
                url = "{{profile().host}}/anything/{{profile().user_guid}}",
                authentication = authentication,
                headers = headers,
            },

            {
                id = "modify_user",
                type = "request",
                name = "Modify User",
                method = "PUT",
                url = "{{profile().host}}/anything/{{profile().user_guid}}",
                authentication = authentication,
                headers = headers,
                body = {
                    type = "json",
                    data = {
                        new_username = "user formerly known as {{username()}}",
                        number = 3,
                        bool = true,
                        null = nil,
                        array = { 1, 2, false, 3.3, "www.www" }
                    }
                }
            }
        }
    },

    {
        id = "get_image",
        type = "request",
        headers = { Accept = "image/png" },
        name = "Get Image",
        method = "GET",
        url = "{{profile().host}}/image"
    },

    {
        id = "upload_image",
        type = "request",
        name = "Upload Image",
        method = "POST",
        url = "{{profile().host}}/anything/image",
        body = {
            type = "form_multipart",
            data = {
                filename = "logo.png",
                image = "{{file({path = './static/png'})}}"
            }
        }
    },

    {
        id = "big_file",
        type = "request",
        name = "Big File",
        method = "POST",
        url = "{{profile().host}}/anything",
        body = {
            type = "json",
            data = {file_content = "{{big_file()}}"}
        }
    },

    {
        id = "delay",
        type = "request",
        name = "Delay",
        method = "GET",
        url = "{{profile().host}}/delay/5",
        authentication = authentication,
        headers = headers,
    }
}

collection = {profiles = profiles, requests = requests}
