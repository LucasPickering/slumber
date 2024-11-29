profiles {
  works {
    name    = "This Works"
    default = true
    data = {
      host      = "https://httpbin.org"
      username  = "xX${locals.username}Xx"
      user_guid = "abc123"
    }
  }
  init-fails {
    name = "Request Init Fails"
    data = {}
  }
  request-fails {
    name = "Request Fails"
    data = {
      host      = "http://localhost:5000"
      username  = "xX${locals.username}Xx"
      user_guid = "abc123"
    }
  }
}

locals {
  authentication = {
    type = "bearer"
    token = json_path({
      query = "$.form",
      data  = response({ recipe = "login" })
    })
  }
  headers = {
    Accept         = "application/json"
    "Content-Type" = "application/json"
  }
  login_form_values = json_path({
    query = "$.form[*]"
    data  = response({ recipe = "login" })
  })
  username = command({ command = ["whoami"], trim = "both" })
  password = prompt({ message = "Password" })
}

requests {
  login "request" {
    method = "POST"
    url    = "${host}/anything/login"
    authentication "basic" {
      username = profile.username
      password = locals.password
    }
    query = [
      "sudo=yes_please",
      "fast=no_thanks",
      "fast=actually_maybe"
    ]
    headers = {
      Accept = "application/json"
    }

    body "form_urlencoded" {
      username = profile.username
      password = locals.password
    }
  }

  users "folder" {
    name = "Users"
    requests {
      get_users "request" {
        name           = "Get Users"
        method         = "GET"
        url            = "${host}/get"
        authentication = locals.authentication
        query = [
          "foo=bar",
          "select=${select({ message = "select a value", options = locals.login_form_values })}"
        ]
      }

      get_user "request" {
        name           = "Get User"
        method         = "GET"
        url            = "${host}/anything/${user_guid}"
        authentication = locals.authentication
        headers        = locals.headers
      }

      modify_user "request" {
        name           = "Modify User"
        method         = "PUT"
        url            = "${host}/anything/${user_guid}"
        authentication = locals.authentication
        headers        = locals.headers
        # body "json" {
        #   new_username = "user formerly known as ${locals.username}"
        #   number       = 3
        #   bool         = true
        #   null         = null
        #   array        = [1, 2, false, 3.3, "www.www"]
        # }
      }
    }
  }

  get_image "request" {
    name           = "Get Image"
    method         = "GET"
    url            = "${host}/image"
    authentication = locals.authentication
    headers = {
      Accept = "image/png"
    }
  }

  upload_image "request" {
    name           = "Upload Image"
    method         = "POST"
    url            = "${host}/anything/image"
    authentication = locals.authentication
    body "form_multipart" {
      filename = "logo.png"
      image    = file({ path = "./static/slumber.png" })
    }
  }

  big_file "request" {
    name           = "Big File"
    method         = "POST"
    url            = "${host}/anything"
    authentication = locals.authentication
    headers        = locals.headers
    # body "json" {
    #   data = file({path = "Cargo.lock"})
    # }
  }

  delay "request" {
    name           = "Delay"
    method         = "GET"
    url            = "${host}/delay/5"
    authentication = locals.authentication
    headers        = locals.headers
  }
}
